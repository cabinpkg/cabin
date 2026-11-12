-- Canonical registry state. D1 is the source of truth; R2 holds each
-- admitted upload's archive under its own key, uploads/<upload_id>.zip.
--
-- One from-zero baseline on purpose: pre-launch the registry's data is
-- disposable and the operator wipes and re-migrates from zero
-- (cargo registry-wipe; docs/runbook.md, "Data policy"), so schema changes
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
    quota_class TEXT NOT NULL DEFAULT 'default',
    -- Publish token-bucket state, NULL until the user's first publish:
    -- rl_tokens is the remaining fractional token count, rl_updated_at
    -- the Unix epoch milliseconds (as text) of the last successful
    -- take. One bucket per user: every token the user holds draws on
    -- it, so minting more tokens buys no more burst.
    rl_tokens REAL,
    rl_updated_at TEXT,
    -- Exact tallies of the rows the per-user quotas count: packages
    -- with created_by = id, and archive_size summed over revisions with
    -- published_by = id that are not rejected. Every batch that writes
    -- such a row adjusts them in the same transaction
    -- (src/sql/admission.rs, src/sql/quota.rs), so the publish preflight
    -- reads one row instead of the account's whole history
    -- (docs/runbook.md, "Quota tallies").
    package_count INTEGER NOT NULL DEFAULT 0,
    stored_bytes INTEGER NOT NULL DEFAULT 0
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

-- One row per live browser sign-in (src/session.rs). The id is random
-- and sealed into the session cookie with the GitHub account id; the
-- session plane authenticates a cookie only while its row exists, so
-- logout deletes exactly that row and a copied cookie stops working
-- with it. expires_at (Unix seconds) is the cookie's own sealed expiry,
-- kept only so the sign-in batch can prune dead rows.
CREATE TABLE browser_sessions (
    id TEXT PRIMARY KEY NOT NULL,
    provider_account_id TEXT NOT NULL,
    expires_at INTEGER NOT NULL
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
    -- Required (non-NULL) for every kind by the shape CHECK
    -- constraints below: no token stands forever. Same ISO-8601 UTC
    -- text shape as created_at
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
    -- NULL inherits the owning user's users.quota_class (the session
    -- mint never writes this column). The trustpub exchange persists
    -- its config's granted tier here so the grant rides the
    -- short-lived token instead of rewriting the backing user's
    -- standing class; the auth lookup coalesces token-first.
    quota_class TEXT,
    -- Closed domain like scope_members.role: 'trustpub' tokens are the
    -- short-lived product of a GitHub Actions OIDC exchange, 'session'
    -- tokens are the CLI login's short-lived human credential. Every
    -- kind is machine-minted and expiring; no standing credential
    -- exists, and both mints write the column explicitly (no DEFAULT).
    kind TEXT NOT NULL CHECK (kind IN ('trustpub', 'session')),
    -- Short-lived and exactly one of two shapes is what 'trustpub'
    -- MEANS, and the schema enforces both so a bug in a minting path
    -- cannot widen an exchange into a standing or over-privileged
    -- credential. The publish shape (the config arm) is confined to
    -- its config's scope and carries the config's granted quota
    -- class; the verify shape (the pinned verifier arm) is
    -- unconfined and unclassed - NULL quota_class inherits the
    -- backing user's class - because the verify scope reads the
    -- admin plane and drives the governor, never per-scope writes.
    -- A verify-scoped token still cannot deliver a verdict: the
    -- verdict route takes no registry token at all, only a per-PATCH
    -- OIDC JWT bound to the VERIFIER_AUDIENCE. The expiry sits
    -- within one day of issuance - a CEILING, not the policy; the
    -- exchange sets the real TTL in code, the schema only refuses a
    -- mint that could not belong to any legitimate exchange window.
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
            AND (
                (
                    scopes = 'publish'
                    AND scope_limit IS NOT NULL AND scope_limit != ''
                    AND quota_class IS NOT NULL
                )
                OR (
                    scopes = 'verify'
                    AND scope_limit IS NULL
                    AND quota_class IS NULL
                )
            )
        )
    ),
    -- The 'session' shape, enforced for the same reason as trustpub's:
    -- a session is the short-lived credential a signed-in human holds,
    -- and a bug in the minting path must not widen it into a standing or
    -- confined credential. Twelve hours is the TTL the mint sets in code
    -- (src/session_tokens.rs); the schema reuses trustpub's one-day
    -- ceiling with the same fail-closed ifnull anchor. The scope set is
    -- publish,yank, with verify only for the operator's own account (the
    -- mint keys it on the seeded operator row, users.id 1), and NULL
    -- quota_class inherits the owning user's class live through the auth
    -- lookup's COALESCE.
    CHECK (
        kind != 'session'
        OR (
            expires_at IS NOT NULL
            AND expires_at <=
                ifnull(strftime('%Y-%m-%dT%H:%M:%fZ', created_at, '+1 day'), '')
            AND scopes IN ('publish,yank', 'publish,yank,verify')
            AND scope_limit IS NULL
            AND quota_class IS NULL
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

-- Replay protection for GitHub OIDC JWTs, shared across every
-- audience the registry accepts (GitHub mints jti values unique
-- across audiences): a JWT's jti is consumed exactly once (INSERT
-- into the primary key; the loser of a race fails). NOT NULL is
-- load-bearing on a TEXT primary key: SQLite permits duplicate
-- NULLs in a non-INTEGER PRIMARY KEY, which would exempt a null
-- jti from the once-only rule. expires_at is the end of the
-- JWT's acceptance window (src/trustpub.rs
-- GithubClaims::verifiable_until: the token's `exp` PLUS the
-- verification leeway, Unix seconds) so rows can be pruned once the
-- JWT they name could no longer verify anyway - storing raw `exp`
-- would reopen replay inside the leeway.
CREATE TABLE oidc_used_jtis (
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

-- The operator's own account, pre-promoted so a wipe never resets it
-- to 'default' (docs/architecture.md, "Quota classes"). Keyed by the
-- immutable numeric GitHub id like the trustpub seed above
-- (26405363 = ken-matsui); `login_snapshot` is display-only and
-- refreshed at every sign-in. Sign-in finds this identity and binds
-- to this row instead of creating one, so the class survives.
-- users.id 1 is explicit and deliberate: the operator is user 1.
INSERT INTO users (id, created_at, quota_class)
VALUES (1, '2026-08-27T00:00:00.000Z', 'operator');
INSERT INTO identities (provider, provider_account_id, login_snapshot, user_id)
VALUES ('github', '26405363', 'ken-matsui', 1);

-- `created_by` / `published_by` hold the registry-native users.id as
-- real foreign keys - attribution is always written explicitly, and a
-- provider account id (or any other stray number) can never enter
-- these tables.
CREATE TABLE packages (
    scope TEXT NOT NULL REFERENCES scopes,
    name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    created_by INTEGER NOT NULL REFERENCES users,
    -- The registry's generation (meta, 'catalog_generation') as of the
    -- last change to what the package's document says; 0 until one of
    -- its revisions is served.
    generation INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (scope, name)
);
-- created_at serves the created-today quota count as a range scan.
CREATE INDEX packages_created_by ON packages (created_by, created_at);
-- The `-`/`_` twin guards' lookup; the expression must stay identical
-- to theirs (src/sql/quota.rs, src/sql/packages.rs) to be used, and
-- `name` makes it covering, without which the planner prefers scanning
-- the whole scope through the primary key.
CREATE INDEX packages_folded_name ON packages (scope, REPLACE(name, '_', '-'), name);

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
--
-- The row is also the record of the upload operation that admitted it
-- (docs/remote-registry.md, "Publish"); no second ledger exists.
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
    -- resolvable), 'verified' (passed verification; part of the
    -- registry, and immutable, once archived_at is set), or 'rejected'
    -- (never became part of the registry; the revision may be
    -- republished).
    verification TEXT NOT NULL DEFAULT 'pending',
    verification_reason TEXT,
    verified_at TEXT,
    -- When the row turned 'rejected'; NULL otherwise. With verified_at
    -- it dates the terminal state the upload-id prune counts from.
    rejected_at TEXT,
    -- The client's `Cabin-Upload-Id` (32 lowercase hex) of the upload
    -- operation that produced the row's current generation. It names
    -- the stored object, uploads/<upload_id>.zip, and is cleared thirty
    -- days after the row turns terminal.
    upload_id TEXT,
    -- When that operation first charged the daily upload allowances,
    -- and to which quota class. A resumed operation keeps both: it is
    -- never charged twice, and may store only within fourteen days of
    -- charged_at.
    charged_at TEXT,
    charged_class TEXT,
    -- When the archive was confirmed stored in UPLOADS. A 'pending' row
    -- with NULL here is a reservation: admitted and counted, not yet
    -- something the verifier may see.
    stored_at TEXT,
    -- When the archive was confirmed in ARCHIVES under the revision's
    -- own key. A 'verified' row with NULL here passed verification and
    -- is not served yet; the row is part of the registry from the
    -- moment this is set, and it is never cleared.
    archived_at TEXT CHECK (archived_at IS NULL OR verification = 'verified'),
    PRIMARY KEY (scope, name, version, revision),
    FOREIGN KEY (scope, name, version) REFERENCES versions (scope, name, version)
);
CREATE INDEX revisions_published_by ON revisions (published_by);
-- One upload id names one operation.
CREATE UNIQUE INDEX revisions_upload_id ON revisions (upload_id)
    WHERE upload_id IS NOT NULL;
-- The per-user pending limit's count, reservations included.
CREATE INDEX revisions_pending_by_publisher ON revisions (published_by)
    WHERE verification = 'pending';
-- The cron's three sweeps, each a range scan: reservations past their
-- expiry, pending uploads past the verification deadline, and terminal
-- rows whose upload id is due to be cleared.
CREATE INDEX revisions_reservations ON revisions (published_at)
    WHERE verification = 'pending' AND stored_at IS NULL;
CREATE INDEX revisions_pending_stored ON revisions (stored_at)
    WHERE verification = 'pending';
CREATE INDEX revisions_settled_uploads ON revisions (COALESCE(verified_at, rejected_at))
    WHERE upload_id IS NOT NULL;
-- The daily per-package version quota's day count, which the publish
-- preflight and every publish batch statement's quota guard run: a
-- range over the package's revisions of one day, not its history.
CREATE INDEX revisions_published_at ON revisions (scope, name, published_at);
CREATE INDEX revisions_verification ON revisions (verification);
-- The archive placement's work list: verified revisions not yet in
-- ARCHIVES, oldest verdict first.
CREATE INDEX revisions_unarchived ON revisions (verified_at)
    WHERE verification = 'verified' AND archived_at IS NULL;

-- The single definition of "the revision the read plane serves": per
-- version, the verified revision in ARCHIVES with the newest
-- published_at (revision id as the deterministic tie-break).  Every
-- read projection - package documents, search, package detail,
-- reverse dependencies, the source viewer, stats - selects through
-- this view, so "current" can never mean different things on
-- different routes.  Pending and rejected rows, and verified ones
-- whose archive is not in ARCHIVES yet, are invisible here by
-- construction.
CREATE VIEW current_revisions AS
SELECT r.* FROM revisions r
WHERE r.verification = 'verified' AND r.archived_at IS NOT NULL
AND NOT EXISTS (
    SELECT 1 FROM revisions s
    WHERE s.scope = r.scope AND s.name = r.name AND s.version = r.version
    AND s.verification = 'verified' AND s.archived_at IS NOT NULL
    AND (s.published_at > r.published_at
         OR (s.published_at = r.published_at AND s.revision > r.revision))
);

-- One row per change to what a package's document and the catalog
-- say: a revision becoming served, a yank, an un-yank. `generation` is
-- the registry's generation the change produced. The row is open until
-- a publish pass that started 15 minutes or more after `created_at`
-- has seen the INDEX bucket hold the package's document and the
-- catalog, rendered at that generation or a later one.
CREATE TABLE publication_jobs (
    generation INTEGER PRIMARY KEY,
    scope TEXT NOT NULL,
    name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    completed_at TEXT,
    FOREIGN KEY (scope, name) REFERENCES packages (scope, name)
);
-- The publish pass's work list and its completing write.
CREATE INDEX publication_jobs_open ON publication_jobs (scope, name, generation)
    WHERE completed_at IS NULL;

-- The daily upload allowances (docs/remote-registry.md, "Publish"):
-- bytes and upload count admitted per user and per quota class on one
-- UTC day ('YYYY-MM-DD'). A new day is a new row, so nothing resets
-- them; the cron only deletes rows older than yesterday. Charges are
-- never refunded, whatever becomes of the upload.
CREATE TABLE user_daily_upload_usage (
    usage_date TEXT NOT NULL,
    user_id INTEGER NOT NULL REFERENCES users,
    bytes INTEGER NOT NULL,
    uploads INTEGER NOT NULL,
    PRIMARY KEY (usage_date, user_id)
);
CREATE TABLE shared_daily_upload_usage (
    usage_date TEXT NOT NULL,
    quota_class TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    uploads INTEGER NOT NULL,
    PRIMARY KEY (usage_date, quota_class)
);

CREATE TABLE meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- service_mode is the budget-breaker state ('normal' | 'warn' |
-- 'writes_blocked' | 'reads_blocked'; see src/breaker.rs) with its
-- human-readable reason. launched is the data-policy flag
-- (docs/runbook.md, "Data policy"): 'false' while the registry's data
-- is disposable (pre-launch), flipped to 'true' exactly once, by hand,
-- as a launch-checklist item; every destructive maintenance path
-- (cargo registry-launch-guard) reads it first and refuses while 'true'.
-- catalog_generation counts the changes to what the package documents
-- and the catalog say (publication_jobs); it only grows.
INSERT INTO meta (key, value) VALUES
    ('registry_generation', '1'),
    ('catalog_generation', '0'),
    ('service_mode', 'normal'),
    ('service_mode_reason', ''),
    ('launched', 'false');
