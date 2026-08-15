-- Canonical registry state. D1 is the source of truth; R2 only holds
-- immutable, content-addressed archive blobs at blobs/sha256/<hex> -
-- the bare hex tail of the canonical `sha256:<64 lowercase hex>`
-- checksum value; key derivation strips the algorithm prefix.
--
-- One from-zero baseline on purpose: pre-launch the registry's data is
-- disposable and the operator wipes and re-migrates from zero
-- (scripts/wipe.sh; docs/runbook.md, "Data policy"), so schema changes
-- edit this file in place instead of accreting ALTER TABLE layers.
-- Editing it deliberately leaves `migrations-applied` stale, which
-- keeps CI's Worker deploy skipped until the operator wipes/applies
-- and refreshes the stamp (docs/runbook.md, "Deploy skew") - never
-- refresh the stamp in the same change that edits the schema.

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

-- Append-only history of successful claims: one row per grant, written
-- in the claim batch, never updated or deleted. The per-user lifetime
-- claim limit (src/quota.rs, `max_scope_claims_total`) counts these
-- rows, deliberately not `scopes` or `scope_members`: a future release
-- or transfer removes rows there, and giving a name back must never
-- restore claim capacity. No foreign key to `scopes` for the same
-- reason - the history must outlive the scope row.
CREATE TABLE scope_claims (
    scope_name TEXT NOT NULL,
    claimed_by INTEGER NOT NULL REFERENCES users,
    claimed_at TEXT NOT NULL
);
CREATE INDEX scope_claims_claimed_by ON scope_claims (claimed_by);

CREATE TABLE tokens (
    id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users,
    name TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    scopes TEXT NOT NULL,
    -- Canonical toISOString shape, same round-trip CHECK as
    -- expires_at below: created_at is the trustpub ceiling's anchor
    -- (parsed by strftime) AND the auth lookup's not-before bound
    -- (compared lexicographically), and those two views agree only on
    -- canonical text - a parseable non-canonical anchor like a Julian
    -- day string ('+5372750' = year 9997) would satisfy the ceiling
    -- while sorting below the current instant.
    created_at TEXT NOT NULL CHECK (
        length(created_at) = 24
        AND strftime('%Y-%m-%dT%H:%M:%fZ', created_at) IS created_at
    ),
    last_used_at TEXT,
    revoked_at TEXT,
    -- Publish token-bucket state, NULL until the token's first publish:
    -- rl_tokens is the remaining fractional token count, rl_updated_at
    -- the Unix epoch milliseconds (as text) of the last successful
    -- take.
    rl_tokens REAL,
    rl_updated_at TEXT,
    -- NULL never expires. Same ISO-8601 UTC text shape as created_at
    -- (JS toISOString: fixed-width, so lexicographic comparison is the
    -- ordering): the auth lookup enforces expiry with a string
    -- comparison in SQL. The shape is schema-enforced because the
    -- comparison fails OPEN for a malformed value sorting above the
    -- ISO range (e.g. 'z' > any timestamp = a token that never
    -- expires). length() pins the fixed width, and the strftime
    -- round-trip admits only the one canonical render of a real
    -- calendar instant - compared with IS, not =, so an unparsable
    -- value's NULL render is a definite refusal ('2026-99-99...'),
    -- and a normalizable one must re-render byte-identically
    -- (datetime() alone would silently accept '2026-02-31...' as
    -- March 3rd while the lookup compares the stored text; a 24-char
    -- space-separated form parses but re-renders with the 'T').
    -- Deliberately NOT a fixed-width GLOB: D1 caps LIKE/GLOB patterns
    -- at 50 bytes AT EVALUATION, so a digit-position pattern makes
    -- every INSERT into this table fail on D1 with "pattern too
    -- complex" while passing every host-side test - the host suites
    -- pin the D1 limit on their connections so a reintroduction fails
    -- there too (tests/sql_validation.rs).
    expires_at TEXT CHECK (
        expires_at IS NULL
        OR (
            length(expires_at) = 24
            AND strftime('%Y-%m-%dT%H:%M:%fZ', expires_at) IS expires_at
        )
    ),
    -- NULL is an unlimited token. When set, the token may only perform
    -- write-side operations on packages under exactly this scope; the
    -- refusal is the same uniform 403 as a membership miss.
    scope_limit TEXT,
    -- NULL inherits the owning user's users.quota_class (every 'user'
    -- token: the session mint never writes this column). The trustpub
    -- exchange persists its config's granted tier here so the grant
    -- rides the short-lived token instead of rewriting the backing
    -- user's standing class; the auth lookup coalesces token-first.
    quota_class TEXT,
    -- Closed domain like scope_members.role: 'user' tokens are minted
    -- from the website session, 'trustpub' tokens are the short-lived
    -- product of a GitHub Actions OIDC exchange.
    kind TEXT NOT NULL DEFAULT 'user' CHECK (kind IN ('user', 'trustpub')),
    -- Short-lived, confined, and publish-only is what 'trustpub'
    -- MEANS, and the schema enforces all three so a bug in the
    -- minting path cannot widen the exchange into a standing or
    -- privileged credential: scopes = 'publish' exactly (the governor
    -- and verdict planes authorize on the verify scope alone, so a
    -- workflow credential must be unable to hold it), and the expiry
    -- sits within one day of issuance - a CEILING, not the policy;
    -- the exchange sets the real TTL in code, the schema only refuses
    -- a mint that could not belong to any legitimate exchange window.
    -- ifnull makes a malformed created_at anchor fail closed rather
    -- than NULL out of the comparison, and a FORGED far-future anchor
    -- buys nothing: the auth lookup refuses rows before their
    -- created_at, so outrunning the ceiling requires a token that
    -- cannot authenticate until the forged instant arrives.
    CHECK (
        kind != 'trustpub'
        OR (
            expires_at IS NOT NULL
            AND expires_at <=
                ifnull(strftime('%Y-%m-%dT%H:%M:%fZ', created_at, '+1 day'), '')
            AND scope_limit IS NOT NULL AND scope_limit != ''
            AND scopes = 'publish'
            AND quota_class IS NOT NULL
        )
    )
);

-- Trusted Publishing (the crates.io RFC 3691 model): one row per
-- (scope, repository, workflow) a scope's owners have registered as
-- allowed to exchange a GitHub Actions OIDC token for a short-lived
-- 'trustpub' registry token. The repository binding is by numeric
-- GitHub ids, never names: owner logins and repository names can be
-- renamed and reassigned, the ids cannot. git_ref / environment are
-- optional extra claims constraints (NULL matches any). quota_class
-- is the tier the exchange grants when it mints, persisted onto the
-- minted token row's tokens.quota_class - never onto the backing
-- user's users.quota_class, so the grant expires with the token
-- instead of upgrading a standing account. No foreign key to scopes:
-- configs are operator/owner data that must survive scope-membership
-- churn, and the seed below predates any claimed scope row.
CREATE TABLE trustpub_configs (
    id INTEGER PRIMARY KEY,
    scope TEXT NOT NULL,
    repository_owner_id INTEGER NOT NULL,
    repository_id INTEGER NOT NULL,
    workflow_filename TEXT NOT NULL,
    git_ref TEXT,
    environment TEXT,
    quota_class TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (scope, repository_id, workflow_filename)
);
CREATE INDEX trustpub_configs_repository
    ON trustpub_configs (repository_owner_id, repository_id);

-- Replay protection for the OIDC exchange: a JWT's jti is consumed
-- exactly once (INSERT into the primary key; the loser of a race
-- fails). NOT NULL is load-bearing on a TEXT primary key: SQLite
-- permits duplicate NULLs in a non-INTEGER PRIMARY KEY, which would
-- exempt a null jti from the once-only rule. expires_at is the end
-- of the verifier's acceptance window (src/trustpub.rs
-- GithubClaims::verifiable_until: the token's `exp` PLUS the
-- verification leeway, Unix seconds) so rows can be pruned once the
-- JWT they name could no longer verify anyway - storing raw `exp`
-- would reopen replay inside the leeway.
CREATE TABLE trustpub_used_jtis (
    jti TEXT PRIMARY KEY NOT NULL,
    expires_at INTEGER NOT NULL
);

-- The operator-seeded config for the foundation-ports publishing
-- workflow: the ports tree and its publish workflow live in
-- cabinpkg/cabin (.github/workflows/ports-publish.yml), publishing
-- under the cabin-ports scope from main only. The numeric ids are
-- cabinpkg/cabin (119684778) and the cabinpkg organization (35998702).
INSERT INTO trustpub_configs (
    scope, repository_owner_id, repository_id, workflow_filename,
    git_ref, environment, quota_class, created_at
) VALUES (
    'cabin-ports', 35998702, 119684778, 'ports-publish.yml',
    'refs/heads/main', NULL, 'operator', '2026-08-14T00:00:00.000Z'
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

-- The resolution-level unit: one row per published version string.
-- Yank state and the download counter are version-level concepts
-- (resolution excludes yanked versions; downloads aggregate across
-- revisions); everything byte-addressed lives in `revisions`.
CREATE TABLE versions (
    scope TEXT NOT NULL,
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    yanked INTEGER NOT NULL DEFAULT 0,
    -- Cumulative download counter for the artifact read plane
    -- (docs/architecture.md, "Download counts"): one approximate,
    -- monotonically increasing total per version, incremented
    -- best-effort after a verified download's body is served.
    downloads INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (scope, name, version),
    FOREIGN KEY (scope, name) REFERENCES packages (scope, name)
);

-- The immutable unit: one row per (scope, name, version, revision).
-- `checksum` holds the canonical `sha256:<64 lowercase hex>` value
-- every surface serializes; `revision` is the leading 16 characters
-- of its hex tail, so byte-identical republication maps onto the
-- existing row; bytes for a given key never change once the row is
-- pending or verified.
CREATE TABLE revisions (
    scope TEXT NOT NULL,
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    revision TEXT NOT NULL,
    checksum TEXT NOT NULL,
    metadata_json TEXT NOT NULL,
    published_at TEXT NOT NULL,
    archive_size INTEGER NOT NULL,
    published_by INTEGER NOT NULL REFERENCES users,
    -- The asynchronous verification lifecycle (docs/architecture.md,
    -- "The verification lifecycle"): 'pending' (published, not yet
    -- resolvable), 'verified' (part of the registry, immutable), or
    -- 'rejected' (never became part of the registry; its blob is
    -- reclaimed and the revision may be republished).
    verification TEXT NOT NULL DEFAULT 'pending',
    verification_reason TEXT,
    verified_at TEXT,
    PRIMARY KEY (scope, name, version, revision),
    FOREIGN KEY (scope, name, version) REFERENCES versions (scope, name, version)
);
CREATE INDEX revisions_published_by ON revisions (published_by);
-- The checksum index serves the storage self-accounting's
-- first-reference check at publish.
CREATE INDEX revisions_checksum ON revisions (checksum);
CREATE INDEX revisions_verification ON revisions (verification);

-- The single definition of "the revision the read plane serves": per
-- verified version, the verified revision with the newest
-- published_at (revision id as the deterministic tie-break).  Every
-- read projection - package documents, search, package detail,
-- reverse dependencies, the source viewer, stats - selects through
-- this view, so "current" can never mean different things on
-- different routes.  Pending and rejected rows are invisible here by
-- construction.
CREATE VIEW current_revisions AS
SELECT r.* FROM revisions r
WHERE r.verification = 'verified'
AND NOT EXISTS (
    SELECT 1 FROM revisions s
    WHERE s.scope = r.scope AND s.name = r.name AND s.version = r.version
    AND s.verification = 'verified'
    AND (s.published_at > r.published_at
         OR (s.published_at = r.published_at AND s.revision > r.revision))
);

-- The verified-artifact backup queue (see docs/runbook.md, "Disaster
-- recovery"). The verdict batch that marks a version verified enqueues
-- its blob here in the same transaction; the drain (verdict waitUntil
-- plus every breaker cron pass) copies each blob to the BACKUP bucket
-- and deletes the row on success, so a crash between the transition
-- and the copy can never lose the work. Only verified content is ever
-- replicated - pending uploads stay out of the backup set. The breaker
-- cron alerts while rows older than an hour exist.
CREATE TABLE backup_pending (
    key TEXT PRIMARY KEY,
    bytes INTEGER NOT NULL,
    enqueued_at TEXT NOT NULL
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
