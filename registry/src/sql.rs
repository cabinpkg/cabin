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

    /// The bearer-token lookup, joining the owning user's plan; revoked
    /// tokens never match.
    AUTH_TOKEN_LOOKUP =
        "SELECT t.id, t.user_id, t.scopes, u.plan, t.rl_tokens, t.rl_updated_at \
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
        "SELECT i.user_id, i.login_snapshot, u.plan \
         FROM identities i JOIN users u ON u.id = i.user_id \
         WHERE i.provider = ?1 AND i.provider_account_id = ?2";

    // ------------------------------------------------------------------
    // packages/versions: the read plane, publish, yank, verification
    // ------------------------------------------------------------------

    /// The package document's rows: **verified** versions only, so
    /// pending and rejected rows never reach composition.
    VERIFIED_VERSIONS_BY_NAME =
        "SELECT version, metadata_json, yanked FROM versions \
         WHERE name = ?1 AND verification = 'verified'";

    /// The yank handler's current-state read.
    VERSION_YANK_STATE =
        "SELECT yanked, verification FROM versions WHERE name = ?1 AND version = ?2";

    /// Applies a yank or un-yank; the `yanked` column is the single home
    /// of yank state.
    SET_VERSION_YANKED = "UPDATE versions SET yanked = ?1 WHERE name = ?2 AND version = ?3";

    /// The verifier's deterministic work list, filtered by status.
    VERSIONS_BY_VERIFICATION_STATUS =
        "SELECT name, version, checksum, published_by, published_at, metadata_json \
         FROM versions WHERE verification = ?1 ORDER BY name, version";

    /// The verdict handler's read of the row a verdict targets.
    VERDICT_TARGET =
        "SELECT verification, checksum, published_at, archive_size FROM versions \
         WHERE name = ?1 AND version = ?2";

    /// Applies a `verified` verdict, guarded on the row still being the
    /// pending generation the verdict was read against.
    MARK_VERSION_VERIFIED =
        "UPDATE versions SET verification = 'verified', verified_at = ?1 \
         WHERE name = ?2 AND version = ?3 \
         AND verification = 'pending' AND checksum = ?4 \
         AND published_at = ?5";

    /// Applies a `rejected` verdict under the same generation guards.
    MARK_VERSION_REJECTED =
        "UPDATE versions SET verification = 'rejected', verification_reason = ?1, \
         verified_at = NULL \
         WHERE name = ?2 AND version = ?3 \
         AND verification = 'pending' AND checksum = ?4 AND published_at = ?5";

    /// The publish handler's idempotency/immutability read of an
    /// existing `(name, version)` row.
    EXISTING_VERSION =
        "SELECT metadata_json, checksum, verification FROM versions \
         WHERE name = ?1 AND version = ?2";

    /// Creates the package row on its first published version. No scope
    /// bind yet, so this cannot satisfy the 0006 schema's `NOT NULL`
    /// scope column at runtime: the scoped-routes step supplies it, and
    /// the registry is pre-launch - this mid-track state is deliberate
    /// and never deployed (see `migrations/0006_identity.sql`).
    INSERT_PACKAGE =
        "INSERT OR IGNORE INTO packages (name, created_at, created_by) \
         VALUES (?1, ?2, ?3)";

    /// Inserts a genuinely new version row, starting `pending`. Same
    /// mid-track caveat as [`INSERT_PACKAGE`]: the scope bind lands
    /// with the scoped-routes step.
    INSERT_VERSION =
        "INSERT INTO versions (name, version, checksum, metadata_json, yanked, \
         published_at, archive_size, published_by, verification) \
         VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7, 'pending')";

    /// Replaces a rejected row in place (back to `pending`), guarded on
    /// the row still being the rejected generation this request read.
    REPLACE_REJECTED_VERSION =
        "UPDATE versions SET checksum = ?1, metadata_json = ?2, yanked = 0, \
         published_at = ?3, archive_size = ?4, published_by = ?5, \
         verification = 'pending', verification_reason = NULL, verified_at = NULL \
         WHERE name = ?6 AND version = ?7 \
         AND verification = 'rejected' AND checksum = ?8";

    /// How many versions have sat `pending` for over an hour (the
    /// stuck-verifier alert).
    COUNT_STALE_PENDING =
        "SELECT COUNT(*) AS n FROM versions WHERE verification = 'pending' \
         AND published_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 hour')";

    /// The session packages listing: every version of every package the
    /// user created, deterministically ordered.
    LIST_USER_PACKAGES =
        "SELECT v.name, v.version, v.verification, v.yanked, v.published_at \
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
        "SELECT COUNT(*) AS n FROM versions WHERE name = ?1 AND published_at >= ?2";

    /// Whether the package row already exists (new-package quotas).
    PACKAGE_EXISTS = "SELECT COUNT(*) AS n FROM packages WHERE name = ?1";

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
    /// reference (see `src/glue.rs`, `persist_new_version`). The CASTs
    /// here and below keep the TEXT-affinity meta value integer-shaped:
    /// D1 binds numbers as floats, and INTEGER + REAL would otherwise
    /// store "254.0", which the breaker's strict u64 parse rejects.
    COUNT_STORED_BYTES_ON_PUBLISH =
        "INSERT INTO meta (key, value) VALUES ('total_stored_bytes', \
         CASE WHEN (SELECT COUNT(*) FROM versions \
                    WHERE checksum = ?1 AND verification != 'rejected') = 1 \
              THEN CAST(?2 AS INTEGER) ELSE 0 END) \
         ON CONFLICT (key) DO UPDATE SET \
         value = CAST(value AS INTEGER) + \
         CASE WHEN (SELECT COUNT(*) FROM versions \
                    WHERE checksum = ?3 AND verification != 'rejected') = 1 \
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
                   WHERE name = ?2 AND version = ?3) = 'pending' \
              AND (SELECT checksum FROM versions \
                   WHERE name = ?2 AND version = ?3) = ?1 \
              AND (SELECT published_at FROM versions \
                   WHERE name = ?2 AND version = ?3) = ?5 \
              THEN CAST(?4 AS INTEGER) ELSE 0 END, 0) \
         WHERE key = 'total_stored_bytes'";

    /// Counts a rejected-replacement's new bytes exactly when the
    /// replacement is about to apply and no other live row references
    /// the new checksum (see `src/glue.rs`, `replace_rejected_version`).
    COUNT_STORED_BYTES_ON_REPLACEMENT =
        "UPDATE meta SET value = CAST(value AS INTEGER) + \
         CASE WHEN (SELECT verification FROM versions \
                    WHERE name = ?1 AND version = ?2) = 'rejected' \
              AND (SELECT checksum FROM versions \
                   WHERE name = ?1 AND version = ?2) = ?3 \
              AND (SELECT COUNT(*) FROM versions \
                   WHERE checksum = ?4 AND verification != 'rejected') = 0 \
              THEN CAST(?5 AS INTEGER) ELSE 0 END \
         WHERE key = 'total_stored_bytes'";

    // ------------------------------------------------------------------
    // backup: blob-replication failure bookkeeping
    // ------------------------------------------------------------------

    /// Clears one key's replication-failure record (successful copy, or
    /// the blob no longer needs a backup).
    CLEAR_REPLICATION_FAILURE = "DELETE FROM backup_replication_failures WHERE key = ?1";

    /// Records (or refreshes) one key's replication failure for
    /// `scripts/backup-backfill.sh`.
    RECORD_REPLICATION_FAILURE =
        "INSERT INTO backup_replication_failures (key, failed_at) \
         VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET \
         failed_at = excluded.failed_at";

    /// How many replication failures are outstanding (the breaker's
    /// backup-health alert).
    COUNT_REPLICATION_FAILURES = "SELECT COUNT(*) AS n FROM backup_replication_failures";

    // ------------------------------------------------------------------
    // downloads: the artifact read plane and blob reclaim
    // ------------------------------------------------------------------

    /// The artifact route's checksum and read-gate lookup.
    ARTIFACT_BY_NAME_VERSION =
        "SELECT checksum, verification FROM versions WHERE name = ?1 AND version = ?2";

    /// Live (non-rejected) references to one blob, for reclaim.
    COUNT_LIVE_BLOB_REFERENCES =
        "SELECT COUNT(*) AS n FROM versions \
         WHERE checksum = ?1 AND verification != 'rejected'";
}
