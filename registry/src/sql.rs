//! Every SQL statement the Worker executes, in one place. (Operational
//! scripts under `scripts/` run their own SQL through `wrangler d1`;
//! this module owns the service's execution paths only.)
//!
//! All execution goes through D1 `prepare`, and every runtime value
//! rides a `?N` bind - parameterization is what injection safety rests
//! on; the few fixed queries take no input at all. These consts are the
//! single home
//! of the executed strings so the host-target validation test
//! (`tests/sql_validation.rs`) can prepare each one against the real,
//! from-zero migrated schema - catching typos, wrong column names, and
//! schema drift at test time - and so the CI guard
//! (`scripts/check-sql.sh`) can keep new call sites from bypassing it.
//! See `docs/architecture.md`, "Why no ORM".

/// Declares one documented `pub const` per statement and collects every
/// statement into [`ALL`], so the validation test cannot silently miss
/// one. `literal` (not `expr`) on purpose: computed SQL has no business
/// here.
macro_rules! statements {
    ($($(#[$doc:meta])* $name:ident = $sql:literal;)+) => {
        $($(#[$doc])* pub const $name: &str = $sql;)+

        /// Every executed statement, for `tests/sql_validation.rs`; the
        /// deployed Worker only ever uses the individual consts.
        #[cfg(not(target_arch = "wasm32"))]
        pub static ALL: &[&str] = &[$($name),+];
    };
}

statements! {
    // ------------------------------------------------------------------
    // auth/tokens: bearer-token verification, token management, users
    // ------------------------------------------------------------------

    /// The bearer-token lookup, joining the owning user's quota class;
    /// revoked tokens never match.
    AUTH_TOKEN_LOOKUP =
        "SELECT t.id, t.user_id, t.scopes, u.quota_class, t.rl_tokens, t.rl_updated_at \
         FROM tokens t JOIN users u ON u.id = t.user_id \
         WHERE t.token_hash = ?1 AND t.revoked_at IS NULL";

    /// Best-effort `last_used_at` bookkeeping on every
    /// bearer-authenticated request.
    TOUCH_TOKEN_LAST_USED = "UPDATE tokens SET last_used_at = ?1 WHERE id = ?2";

    /// The session token listing: metadata only, never hashes.
    LIST_USER_TOKENS =
        "SELECT id, name, scopes, created_at, last_used_at, revoked_at \
         FROM tokens WHERE user_id = ?1 ORDER BY created_at DESC, id";

    /// Issues a token; D1 stores only the SHA-256 hex of the plaintext.
    INSERT_TOKEN =
        "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)";

    /// Revokes one of the session user's own tokens; first `revoked_at`
    /// wins.
    REVOKE_TOKEN =
        "UPDATE tokens SET revoked_at = ?1 \
         WHERE id = ?2 AND user_id = ?3 AND revoked_at IS NULL";

    /// Creates the registry-native user row exactly when the identity
    /// is new. Must run in one batch (one transaction) directly before
    /// [`UPSERT_IDENTITY`], which reads `last_insert_rowid()` from this
    /// statement's insert.
    INSERT_USER_FOR_NEW_IDENTITY =
        "INSERT INTO users (created_at) \
         SELECT ?1 WHERE NOT EXISTS \
         (SELECT 1 FROM identities WHERE provider = ?2 AND provider_account_id = ?3)";

    /// Binds a new identity to the user row the batch just created,
    /// refreshing the display login on every sign-in. When the identity
    /// already exists, `last_insert_rowid()` is stale - the preceding
    /// statement inserted nothing - and the DO UPDATE discards it: only
    /// `login_snapshot` is ever rewritten, the user binding is
    /// immutable.
    UPSERT_IDENTITY =
        "INSERT INTO identities (provider, provider_account_id, login_snapshot, user_id) \
         VALUES (?1, ?2, ?3, last_insert_rowid()) \
         ON CONFLICT (provider, provider_account_id) \
         DO UPDATE SET login_snapshot = excluded.login_snapshot";

    /// The session's user resolution: the sealed cookie names the
    /// external identity, resolved to the registry-native user row on
    /// every request.
    USER_BY_IDENTITY =
        "SELECT i.user_id, i.login_snapshot, u.quota_class \
         FROM identities i JOIN users u ON u.id = i.user_id \
         WHERE i.provider = ?1 AND i.provider_account_id = ?2";

    // ------------------------------------------------------------------
    // scopes: the claim flow and membership management
    // ------------------------------------------------------------------

    /// The claim callback's pre-check: claims are permanent, so an
    /// existing row refuses whoever asks.
    SCOPE_EXISTS = "SELECT COUNT(*) AS n FROM scopes WHERE name = ?1";

    /// Claims a scope. Deliberately a plain INSERT: `name` is the
    /// primary key, so the loser of a claim race fails the statement,
    /// which rolls back its whole batch - [`SEED_CLAIM_OWNER`] must run
    /// in that same batch, so a lost race can never seed the loser as
    /// an owner of the winner's scope.
    CLAIM_SCOPE =
        "INSERT INTO scopes (name, proof_provider, proof_account_id, claimed_at) \
         VALUES (?1, ?2, ?3, ?4)";

    /// Seeds the claiming user as the new scope's first owner, in the
    /// same batch as [`CLAIM_SCOPE`].
    SEED_CLAIM_OWNER =
        "INSERT INTO scope_members (scope_name, user_id, role) VALUES (?1, ?2, 'owner')";

    /// Every claimed scope name, for the claim callback's skeleton
    /// confusability refusal (`docs/architecture.md`, "Name
    /// fidelity"): the fold runs in Rust (`crate::names::skeleton`),
    /// so the map lives in one place per crate instead of a second
    /// SQL spelling. Scopes are few; the breaker's `d1_rows_read_day`
    /// budget is the tripwire if that stops holding.
    LIST_SCOPE_NAMES = "SELECT name FROM scopes ORDER BY name";

    /// Whether the user holds the `owner` role in the scope: the gate on
    /// every membership-management endpoint. A scope that does not exist
    /// has no owners, so nonexistent and foreign scopes answer
    /// identically, mirroring [`SCOPE_MEMBERSHIP`].
    SCOPE_OWNER_MEMBERSHIP =
        "SELECT COUNT(*) AS n FROM scope_members \
         WHERE scope_name = ?1 AND user_id = ?2 AND role = 'owner'";

    /// The members listing, resolved back to the external identity the
    /// management API speaks (the provider bind is policy's `github`).
    /// Ordered by the stable registry user id for determinism.
    LIST_SCOPE_MEMBERS =
        "SELECT i.provider_account_id, i.login_snapshot, sm.role \
         FROM scope_members sm \
         JOIN identities i ON i.user_id = sm.user_id AND i.provider = ?2 \
         WHERE sm.scope_name = ?1 ORDER BY sm.user_id";

    /// One member's current role, if any (shapes the add/remove
    /// responses).
    SCOPE_MEMBER_ROLE =
        "SELECT role FROM scope_members WHERE scope_name = ?1 AND user_id = ?2";

    /// Adds a member; an existing membership keeps its role (there is no
    /// role-change endpoint, and an upsert here could demote the last
    /// owner).
    ADD_SCOPE_MEMBER =
        "INSERT OR IGNORE INTO scope_members (scope_name, user_id, role) \
         VALUES (?1, ?2, ?3)";

    /// Removes a member unless that would leave the scope ownerless: the
    /// last-owner rule is enforced inside the statement, so concurrent
    /// removals cannot race past it.
    REMOVE_SCOPE_MEMBER =
        "DELETE FROM scope_members WHERE scope_name = ?1 AND user_id = ?2 \
         AND (role != 'owner' OR \
              (SELECT COUNT(*) FROM scope_members \
               WHERE scope_name = ?1 AND role = 'owner') > 1)";

    // ------------------------------------------------------------------
    // packages/versions: the read plane, publish, yank, verification
    // ------------------------------------------------------------------

    /// The package document's rows: **verified** versions only, so
    /// pending and rejected rows never reach composition.
    VERIFIED_VERSIONS_BY_PACKAGE =
        "SELECT version, metadata_json, yanked FROM versions \
         WHERE scope = ?1 AND name = ?2 AND verification = 'verified'";

    /// The yank handler's current-state read.
    VERSION_YANK_STATE =
        "SELECT yanked, verification FROM versions \
         WHERE scope = ?1 AND name = ?2 AND version = ?3";

    /// Applies a yank or un-yank; the `yanked` column is the single home
    /// of yank state.
    SET_VERSION_YANKED =
        "UPDATE versions SET yanked = ?1 WHERE scope = ?2 AND name = ?3 AND version = ?4";

    /// The verifier's deterministic work list, filtered by status.
    VERSIONS_BY_VERIFICATION_STATUS =
        "SELECT scope, name, version, checksum, published_by, published_at, metadata_json \
         FROM versions WHERE verification = ?1 ORDER BY scope, name, version";

    /// The admin corpus listing (`docs/architecture.md`, "Name
    /// fidelity"): every package with whether any of its versions is
    /// **verified** - the verifier's name advisories compare a
    /// candidate against every existing name, and skip a candidate
    /// whose name was accepted once. Deliberately verified-only, not
    /// any-verdict: a rejection must never vet a name, or an operator
    /// rejecting an abstained squat would exempt that very name's
    /// next version from the advisories.
    ADMIN_PACKAGES =
        "SELECT p.scope, p.name, \
         EXISTS(SELECT 1 FROM versions v \
                WHERE v.scope = p.scope AND v.name = p.name \
                AND v.verification = 'verified') AS vetted \
         FROM packages p ORDER BY p.scope, p.name";

    /// The verdict handler's read of the row a verdict targets.
    VERDICT_TARGET =
        "SELECT verification, checksum, published_at, archive_size FROM versions \
         WHERE scope = ?1 AND name = ?2 AND version = ?3";

    /// Applies a `verified` verdict, guarded on the row still being the
    /// pending generation the verdict was read against.
    MARK_VERSION_VERIFIED =
        "UPDATE versions SET verification = 'verified', verified_at = ?1 \
         WHERE scope = ?2 AND name = ?3 AND version = ?4 \
         AND verification = 'pending' AND checksum = ?5 \
         AND published_at = ?6";

    /// Applies a `rejected` verdict under the same generation guards.
    MARK_VERSION_REJECTED =
        "UPDATE versions SET verification = 'rejected', verification_reason = ?1, \
         verified_at = NULL \
         WHERE scope = ?2 AND name = ?3 AND version = ?4 \
         AND verification = 'pending' AND checksum = ?5 AND published_at = ?6";

    /// The publish handler's idempotency/immutability read of an
    /// existing `(scope, name, version)` row.
    EXISTING_VERSION =
        "SELECT metadata_json, checksum, verification FROM versions \
         WHERE scope = ?1 AND name = ?2 AND version = ?3";

    /// Whether the token's user is a member (any role) of the scope: the
    /// write plane's authorization read. A scope that does not exist has
    /// no members, so nonexistent and foreign scopes answer identically
    /// by construction (`docs/architecture.md`, "The write path").
    SCOPE_MEMBERSHIP =
        "SELECT COUNT(*) AS n FROM scope_members WHERE scope_name = ?1 AND user_id = ?2";

    /// Creates the package row on its first published version - unless
    /// that would create a `-`/`_` twin of an existing same-scope
    /// package ([`TWIN_PACKAGE_EXISTS`] is the preflight that renders
    /// the `400`; this in-batch guard closes the race between two
    /// concurrent twin publishes, whose preflights both saw neither).
    INSERT_PACKAGE =
        "INSERT OR IGNORE INTO packages (scope, name, created_at, created_by) \
         SELECT ?1, ?2, ?3, ?4 WHERE NOT EXISTS \
         (SELECT 1 FROM packages WHERE scope = ?1 AND name != ?2 \
          AND REPLACE(name, '_', '-') = REPLACE(?2, '_', '-'))";

    /// Inserts a genuinely new version row, starting `pending`,
    /// guarded on its own package row existing. The batch runs
    /// [`INSERT_PACKAGE`] first, so after it the row is absent exactly
    /// when the twin guard suppressed a new package - zero changed
    /// rows here means a twin won the race and nothing was persisted
    /// (the glue answers the twin `400`) - while an already-existing
    /// package always passes, twin or not: the twin policy gates
    /// package creation only, never new versions of what exists.
    INSERT_VERSION =
        "INSERT INTO versions (scope, name, version, checksum, metadata_json, yanked, \
         published_at, archive_size, published_by, verification) \
         SELECT ?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8, 'pending' WHERE EXISTS \
         (SELECT 1 FROM packages WHERE scope = ?1 AND name = ?2)";

    /// Replaces a rejected row in place (back to `pending`), guarded on
    /// the row still being the rejected generation this request read.
    REPLACE_REJECTED_VERSION =
        "UPDATE versions SET checksum = ?1, metadata_json = ?2, yanked = 0, \
         published_at = ?3, archive_size = ?4, published_by = ?5, \
         verification = 'pending', verification_reason = NULL, verified_at = NULL \
         WHERE scope = ?6 AND name = ?7 AND version = ?8 \
         AND verification = 'rejected' AND checksum = ?9";

    /// How many versions have sat `pending` for over an hour (the
    /// stuck-verifier alert).
    COUNT_STALE_PENDING =
        "SELECT COUNT(*) AS n FROM versions WHERE verification = 'pending' \
         AND published_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 hour')";

    /// The dashboard search's row set: every verified version of every
    /// package whose canonical `<scope>/<name>` name contains the term
    /// as a literal substring. `instr`, deliberately not a `LIKE`
    /// pattern: D1 caps `LIKE`/`GLOB` patterns at 50 bytes, which
    /// would refuse valid terms under the documented 64-character
    /// contract, while `instr` takes the term verbatim - nothing to
    /// escape, no wildcards to smuggle. It compares bytes exactly;
    /// the caller ASCII-lowercases the term, and names are lowercase
    /// by grammar, so the match is ASCII-case-insensitive. Grouping,
    /// ranking, and the result limit happen in host-testable Rust
    /// (`user_api::search_json`), so the statement stays a plain
    /// verified-only filter. Like [`REVERSE_DEPENDENCIES`], this scans
    /// the verified corpus per call - accepted at current scale; the
    /// breaker's `d1_rows_read_day` budget is the tripwire.
    SEARCH_VERIFIED_VERSIONS =
        "SELECT scope, name, version, yanked, published_at, downloads \
         FROM versions WHERE verification = 'verified' \
         AND instr(scope || '/' || name, ?1) > 0";

    /// One visible package's verified versions with the stored
    /// metadata each carries: the session package-detail read.
    /// Verified-only like [`VERIFIED_VERSIONS_BY_PACKAGE`], so a
    /// package with none is a missing package by construction.
    VERIFIED_VERSION_DETAILS =
        "SELECT version, metadata_json, yanked, published_at, downloads \
         FROM versions WHERE scope = ?1 AND name = ?2 AND verification = 'verified'";

    /// Whether the package is visible at all (>= 1 verified version):
    /// the reverse-dependencies target gate.
    HAS_VERIFIED_VERSION =
        "SELECT COUNT(*) AS n FROM versions \
         WHERE scope = ?1 AND name = ?2 AND verification = 'verified'";

    /// The verified versions whose stored `dependencies` map contains
    /// the canonical `<scope>/<name>` key in `?1`: a `json_each` walk
    /// over every verified row's `metadata_json` per call. That full
    /// scan is the recorded decision (`docs/architecture.md`, "Search
    /// and reverse dependencies"): at current scale it sits well
    /// inside the D1 budget the breaker watches (`d1_rows_read_day`),
    /// and the upgrade path - a publish-maintained dependents table,
    /// the crates.io approach - is to be taken only if that metric
    /// warns, not preemptively. Dependent packages are visible by
    /// construction: a verified matching version is itself the
    /// dependent's visibility.
    REVERSE_DEPENDENCIES =
        "SELECT scope, name, version, published_at FROM versions \
         WHERE verification = 'verified' \
         AND EXISTS (SELECT 1 FROM json_each(versions.metadata_json, '$.dependencies') \
                     WHERE json_each.key = ?1)";

    /// The session packages listing: every version of every package the
    /// user created, deterministically ordered, each with its served-
    /// download count (the dashboard's per-package figures).
    LIST_USER_PACKAGES =
        "SELECT v.scope, v.name, v.version, v.verification, v.yanked, v.published_at, \
         v.downloads \
         FROM packages p JOIN versions v ON v.scope = p.scope AND v.name = p.name \
         WHERE p.created_by = ?1 \
         ORDER BY v.scope, v.name, v.published_at DESC, v.version";

    // ------------------------------------------------------------------
    // quota: the publish rate limit and the per-user quota counts
    // ------------------------------------------------------------------

    /// The current token-bucket state straight from the token row.
    TOKEN_BUCKET = "SELECT rl_tokens, rl_updated_at FROM tokens WHERE id = ?1";

    /// Persists a bucket take iff the row still holds the state the take
    /// was computed from (`IS` keeps the comparison NULL-safe).
    CAS_TOKEN_BUCKET =
        "UPDATE tokens SET rl_tokens = ?1, rl_updated_at = ?2 \
         WHERE id = ?3 AND rl_tokens IS ?4 AND rl_updated_at IS ?5";

    /// The publisher's stored bytes; rejected rows were refunded.
    USER_STORED_BYTES =
        "SELECT COALESCE(SUM(archive_size), 0) AS stored_bytes \
         FROM versions WHERE published_by = ?1 AND verification != 'rejected'";

    /// The creator's total and created-today package counts.
    USER_PACKAGE_COUNTS =
        "SELECT COUNT(*) AS package_count, \
         COALESCE(SUM(created_at >= ?2), 0) AS new_today \
         FROM packages WHERE created_by = ?1";

    /// Versions published into one package since a cutoff (the daily
    /// per-package quota).
    COUNT_PACKAGE_VERSIONS_SINCE =
        "SELECT COUNT(*) AS n FROM versions \
         WHERE scope = ?1 AND name = ?2 AND published_at >= ?3";

    /// Whether the package row already exists (new-package quotas).
    PACKAGE_EXISTS = "SELECT COUNT(*) AS n FROM packages WHERE scope = ?1 AND name = ?2";

    /// Whether creating `(scope, name)` would collide with an existing
    /// same-scope package under `-`/`_` folding: the deterministic
    /// publish reject (`docs/architecture.md`, "Name fidelity").
    /// `REPLACE` in the query, not a normalized column - the packages
    /// table is small and this runs once per prospective publish. The
    /// self-exclusion keeps the predicate identical to the in-batch
    /// guards on [`INSERT_PACKAGE`] / [`INSERT_VERSION`].
    TWIN_PACKAGE_EXISTS =
        "SELECT COUNT(*) AS n FROM packages WHERE scope = ?1 AND name != ?2 \
         AND REPLACE(name, '_', '-') = REPLACE(?2, '_', '-')";

    /// The dashboard usage aggregate over everything the user published.
    USER_USAGE =
        "SELECT COALESCE(SUM(CASE WHEN verification != 'rejected' \
         THEN archive_size ELSE 0 END), 0) AS stored_bytes, \
         COALESCE(SUM(CASE WHEN published_at >= ?2 THEN 1 ELSE 0 END), 0) AS published_today, \
         COALESCE(SUM(verification = 'verified'), 0) AS verified_count, \
         COALESCE(SUM(verification = 'pending'), 0) AS pending_count, \
         COALESCE(SUM(verification = 'rejected'), 0) AS rejected_count \
         FROM versions WHERE published_by = ?1";

    /// The dashboard's created-package count (quota semantics: created,
    /// not merely published into).
    USER_CREATED_PACKAGE_COUNT = "SELECT COUNT(*) AS n FROM packages WHERE created_by = ?1";

    // ------------------------------------------------------------------
    // meta: service state and the storage self-accounting
    // ------------------------------------------------------------------

    /// The pre-launch debug header's generation stamp.
    REGISTRY_GENERATION = "SELECT value FROM meta WHERE key = 'registry_generation'";

    /// One `meta` row by key.
    META_VALUE = "SELECT value FROM meta WHERE key = ?1";

    /// Upserts one `meta` row.
    UPSERT_META =
        "INSERT INTO meta (key, value) VALUES (?1, ?2) \
         ON CONFLICT (key) DO UPDATE SET value = excluded.value";

    /// Counts a published archive's bytes into `total_stored_bytes`
    /// exactly when the just-inserted row is the checksum's sole live
    /// reference (see `src/glue.rs`, `persist_new_version`). The
    /// row-exists conjunct pins "just-inserted" to this batch's own
    /// [`INSERT_VERSION`]: when its twin guard suppressed the insert,
    /// the sole live reference is a racing twin's row and must not be
    /// counted again. The CASTs
    /// here and below keep the TEXT-affinity meta value integer-shaped:
    /// D1 binds numbers as floats, and INTEGER + REAL would otherwise
    /// store "254.0", which the breaker's strict u64 parse rejects.
    COUNT_STORED_BYTES_ON_PUBLISH =
        "INSERT INTO meta (key, value) VALUES ('total_stored_bytes', \
         CASE WHEN (SELECT COUNT(*) FROM versions \
                    WHERE checksum = ?1 AND verification != 'rejected') = 1 \
              AND EXISTS (SELECT 1 FROM versions \
                          WHERE scope = ?5 AND name = ?6 AND version = ?7 \
                          AND checksum = ?1) \
              THEN CAST(?2 AS INTEGER) ELSE 0 END) \
         ON CONFLICT (key) DO UPDATE SET \
         value = CAST(value AS INTEGER) + \
         CASE WHEN (SELECT COUNT(*) FROM versions \
                    WHERE checksum = ?3 AND verification != 'rejected') = 1 \
              AND EXISTS (SELECT 1 FROM versions \
                          WHERE scope = ?5 AND name = ?6 AND version = ?7 \
                          AND checksum = ?3) \
              THEN CAST(?4 AS INTEGER) ELSE 0 END";

    /// Refunds a rejected archive's bytes exactly when the row - still
    /// pending, still holding the bytes the verdict was read against -
    /// is the checksum's sole live reference (see `src/glue.rs`,
    /// `apply_rejection`).
    REFUND_STORED_BYTES_ON_REJECTION =
        "UPDATE meta SET value = MAX(CAST(value AS INTEGER) - \
         CASE WHEN (SELECT COUNT(*) FROM versions \
                    WHERE checksum = ?1 AND verification != 'rejected') = 1 \
              AND (SELECT verification FROM versions \
                   WHERE scope = ?2 AND name = ?3 AND version = ?4) = 'pending' \
              AND (SELECT checksum FROM versions \
                   WHERE scope = ?2 AND name = ?3 AND version = ?4) = ?1 \
              AND (SELECT published_at FROM versions \
                   WHERE scope = ?2 AND name = ?3 AND version = ?4) = ?6 \
              THEN CAST(?5 AS INTEGER) ELSE 0 END, 0) \
         WHERE key = 'total_stored_bytes'";

    /// Counts a rejected-replacement's new bytes exactly when the
    /// replacement is about to apply and no other live row references
    /// the new checksum (see `src/glue.rs`, `replace_rejected_version`).
    COUNT_STORED_BYTES_ON_REPLACEMENT =
        "UPDATE meta SET value = CAST(value AS INTEGER) + \
         CASE WHEN (SELECT verification FROM versions \
                    WHERE scope = ?1 AND name = ?2 AND version = ?3) = 'rejected' \
              AND (SELECT checksum FROM versions \
                   WHERE scope = ?1 AND name = ?2 AND version = ?3) = ?4 \
              AND (SELECT COUNT(*) FROM versions \
                   WHERE checksum = ?5 AND verification != 'rejected') = 0 \
              THEN CAST(?6 AS INTEGER) ELSE 0 END \
         WHERE key = 'total_stored_bytes'";

    // ------------------------------------------------------------------
    // backup: the verified-artifact replication queue
    // ------------------------------------------------------------------

    /// Enqueues a just-verified version's blob for backup replication,
    /// in the same batch as [`MARK_VERSION_VERIFIED`]: the row appears
    /// exactly when the verified transition applied (the guards repeat
    /// the mark's), so the queue is recorded transactionally with the
    /// transition and a crash can never lose the work. Shared checksums
    /// collapse onto one queue row; a key already replicated re-enters
    /// harmlessly (the drain's head sees the copy and settles).
    ENQUEUE_VERIFIED_BACKUP =
        "INSERT INTO backup_pending (key, bytes, enqueued_at) \
         SELECT 'blobs/sha256/' || checksum, archive_size, ?6 FROM versions \
         WHERE scope = ?1 AND name = ?2 AND version = ?3 \
         AND verification = 'verified' AND checksum = ?4 AND published_at = ?5 \
         ON CONFLICT (key) DO NOTHING";

    /// The drain's work list: keyset-paginated (`key > ?1`, key
    /// order), so rows a pass must keep (a missing primary blob under
    /// a still-verified version) are walked past instead of pinning
    /// the page - ten stuck rows must not starve every later healthy
    /// entry. The row's `bytes` is deliberately not read here: the
    /// ledger settles at sizes the drain observes (the head's object,
    /// or the buffered copy), never at the enqueue-time expectation.
    LIST_BACKUP_PENDING =
        "SELECT key FROM backup_pending WHERE key > ?1 ORDER BY key LIMIT 10";

    /// Removes one queue row whose work is done (the copy landed).
    DELETE_BACKUP_PENDING = "DELETE FROM backup_pending WHERE key = ?1";

    /// Retires one queue row as dead - but only while no verified
    /// reference exists, re-checked inside the statement: a
    /// check-then-delete split would let a verdict that lands in
    /// between (enqueueing this very key transactionally) lose its
    /// recorded backup work to a stale reader.
    RETIRE_DEAD_BACKUP_PENDING =
        "DELETE FROM backup_pending WHERE key = ?1 \
         AND NOT EXISTS (SELECT 1 FROM versions \
                         WHERE checksum = ?2 AND verification = 'verified')";

    /// Live **verified** references to one blob: the drain only copies
    /// blobs the registry still serves as verified content.
    COUNT_LIVE_VERIFIED_BLOB_REFERENCES =
        "SELECT COUNT(*) AS n FROM versions \
         WHERE checksum = ?1 AND verification = 'verified'";

    /// Queue rows older than an hour (the breaker's backup-health
    /// alert): fresh rows are in-flight work, stale ones mean the
    /// drain is failing or refused.
    COUNT_STALE_BACKUP_PENDING =
        "SELECT COUNT(*) AS n FROM backup_pending \
         WHERE enqueued_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 hour')";

    // ------------------------------------------------------------------
    // downloads: the artifact read plane and blob reclaim
    // ------------------------------------------------------------------

    /// The artifact route's checksum and read-gate lookup.
    ARTIFACT_BY_PACKAGE_VERSION =
        "SELECT checksum, verification FROM versions \
         WHERE scope = ?1 AND name = ?2 AND version = ?3";

    /// The source viewer's lookup: the checksum plus the stored archive
    /// size, which bounds the ranged read before R2 is consulted (the
    /// blob was written from the same bytes the size was recorded
    /// from). The verified filter sits in the query like
    /// [`VERIFIED_VERSIONS_BY_PACKAGE`]'s, so pending, rejected, and
    /// corrupt-status rows are missing rows by construction - sessions
    /// have no verify scope, so unlike the artifact route there is no
    /// pending carve-out to branch on.
    SOURCE_VERSION_LOOKUP =
        "SELECT checksum, archive_size FROM versions \
         WHERE scope = ?1 AND name = ?2 AND version = ?3 \
         AND verification = 'verified'";

    /// The public stats totals: verified packages, verified versions,
    /// and served downloads. `scope || '/' || name` is unambiguous -
    /// `/` is in neither grammar - and a registry with no verified
    /// versions answers all zeros.
    REGISTRY_STATS =
        "SELECT COUNT(DISTINCT scope || '/' || name) AS packages, \
         COUNT(*) AS versions, \
         COALESCE(SUM(downloads), 0) AS downloads \
         FROM versions WHERE verification = 'verified'";

    /// Applies one flush of the batched download telemetry
    /// (`src/telemetry.rs`): the buffered per-version count lands in
    /// one statement per version instead of one write per download.
    /// The `verification` guard keeps the counter honest inside the
    /// statement itself: only verified rows ever count, so the
    /// verifier's pending fetches (readable with the `verify` scope)
    /// and any racing lifecycle change can never increment. Yanked
    /// versions keep counting - they stay downloadable on purpose.
    ADD_VERSION_DOWNLOADS =
        "UPDATE versions SET downloads = downloads + ?4 \
         WHERE scope = ?1 AND name = ?2 AND version = ?3 \
         AND verification = 'verified'";

    /// Live (non-rejected) references to one blob, for reclaim.
    COUNT_LIVE_BLOB_REFERENCES =
        "SELECT COUNT(*) AS n FROM versions \
         WHERE checksum = ?1 AND verification != 'rejected'";

    /// The governor reconciliation's authoritative live set: one size
    /// per distinct live checksum, the same shape the storage
    /// self-accounting counts (`docs/runbook.md`, "Orphaned R2 blobs").
    LIVE_BLOB_SIZES =
        "SELECT checksum, MAX(archive_size) AS size FROM versions \
         WHERE verification != 'rejected' GROUP BY checksum";
}
