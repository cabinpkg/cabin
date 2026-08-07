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
//! (`cargo check-sql`) can keep new call sites from bypassing it.
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

    /// The package document's per-version base rows: each verified
    /// version's **current** revision (the `current_revisions` view is
    /// the single "served revision" definition) with the version-level
    /// yank flag.  Pending and rejected revisions never reach
    /// composition, and a pending respin leaves the previously served
    /// revision in place here.
    CURRENT_REVISIONS_BY_PACKAGE =
        "SELECT c.version, c.revision, c.metadata_json, v.yanked \
         FROM current_revisions c \
         JOIN versions v ON v.scope = c.scope AND v.name = c.name AND v.version = c.version \
         WHERE c.scope = ?1 AND c.name = ?2";

    /// Every **verified** revision of the package, for the composed
    /// document's per-version `revisions` maps: superseded revisions
    /// stay listed (and fetchable) so pinned lockfiles keep building.
    VERIFIED_REVISIONS_BY_PACKAGE =
        "SELECT version, revision, checksum, published_at FROM revisions \
         WHERE scope = ?1 AND name = ?2 AND verification = 'verified' \
         ORDER BY version, revision";

    /// The yank handler's current-state read: the version-level yank
    /// flag plus whether any revision is verified (yank applies to
    /// versions the registry actually serves).
    VERSION_YANK_STATE =
        "SELECT v.yanked, EXISTS(SELECT 1 FROM revisions r \
         WHERE r.scope = v.scope AND r.name = v.name AND r.version = v.version \
         AND r.verification = 'verified') AS verified \
         FROM versions v WHERE v.scope = ?1 AND v.name = ?2 AND v.version = ?3";

    /// Applies a yank or un-yank; the `yanked` column is the single home
    /// of yank state.
    SET_VERSION_YANKED =
        "UPDATE versions SET yanked = ?1 WHERE scope = ?2 AND name = ?3 AND version = ?4";

    /// The verifier's deterministic work list, filtered by status.
    /// One row per revision: a pending respin lists beside the
    /// version's already-verified revisions without disturbing them.
    REVISIONS_BY_VERIFICATION_STATUS =
        "SELECT scope, name, version, revision, checksum, published_by, published_at, \
         metadata_json \
         FROM revisions WHERE verification = ?1 ORDER BY scope, name, version, revision";

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
         EXISTS(SELECT 1 FROM revisions r \
                WHERE r.scope = p.scope AND r.name = p.name \
                AND r.verification = 'verified') AS vetted \
         FROM packages p ORDER BY p.scope, p.name";

    /// The verdict handler's read of the revision row a verdict
    /// targets (the body's required checksum names the revision).
    VERDICT_TARGET =
        "SELECT verification, checksum, published_at, archive_size FROM revisions \
         WHERE scope = ?1 AND name = ?2 AND version = ?3 AND revision = ?4";

    /// Applies a `verified` verdict, guarded on the revision row still
    /// being the pending generation the verdict was read against.
    MARK_REVISION_VERIFIED =
        "UPDATE revisions SET verification = 'verified', verified_at = ?1 \
         WHERE scope = ?2 AND name = ?3 AND version = ?4 AND revision = ?7 \
         AND verification = 'pending' AND checksum = ?5 \
         AND published_at = ?6";

    /// Applies a `rejected` verdict under the same generation guards.
    MARK_REVISION_REJECTED =
        "UPDATE revisions SET verification = 'rejected', verification_reason = ?1, \
         verified_at = NULL \
         WHERE scope = ?2 AND name = ?3 AND version = ?4 AND revision = ?7 \
         AND verification = 'pending' AND checksum = ?5 AND published_at = ?6";

    /// The publish handler's idempotency/immutability read of every
    /// existing revision of `(scope, name, version)`, deterministic
    /// for the re-read after a lost in-batch race.
    EXISTING_REVISIONS =
        "SELECT revision, checksum, verification, published_at FROM revisions \
         WHERE scope = ?1 AND name = ?2 AND version = ?3 ORDER BY revision";

    /// One live (pending or verified) sibling's stored document, for
    /// the publish preflight's resolver-metadata invariance check.
    /// Live siblings agree on the symmetric fields by induction, but
    /// `links` is one-way (a revision may add a table where the
    /// version had none), so siblings can legitimately differ on it -
    /// prefer a links-bearing row, which is authoritative once one
    /// exists.  [`INSERT_REVISION`] re-enforces the same rule inside
    /// the transaction; this read only shapes the diagnostic.
    LIVE_REVISION_METADATA =
        "SELECT metadata_json FROM revisions \
         WHERE scope = ?1 AND name = ?2 AND version = ?3 \
         AND verification IN ('pending', 'verified') \
         ORDER BY (json_extract(metadata_json, '$.links') IS NULL), revision LIMIT 1";

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

    /// Creates the version row on its first published revision,
    /// guarded on its own package row existing (the batch runs
    /// [`INSERT_PACKAGE`] first, so the package is absent exactly when
    /// the twin guard suppressed it).  `INSERT OR IGNORE`: an existing
    /// version row is the respin case, not an error.
    INSERT_VERSION_ROW =
        "INSERT OR IGNORE INTO versions (scope, name, version, yanked, downloads) \
         SELECT ?1, ?2, ?3, 0, 0 WHERE EXISTS \
         (SELECT 1 FROM packages WHERE scope = ?1 AND name = ?2)";

    /// Inserts a genuinely new revision row, starting `pending`.
    /// Four guards are enforced *inside* the transaction, so
    /// concurrent publishes cannot race past the preflight reads:
    /// the version row must exist (zero changed rows after
    /// [`INSERT_VERSION_ROW`] means the twin guard suppressed the
    /// package and nothing was persisted); no row may already hold
    /// this revision id (a racing byte-identical publish whose twin
    /// committed first must lose to the re-read, not fail the
    /// primary key and abort the whole batch); unless `?10` carries
    /// the `new-revision` opt-in, no live (pending or verified)
    /// revision with different bytes may exist; and - opt-in or not
    /// - the resolver-consumed metadata (`dependencies`, `features`,
    /// `standards`) must agree with every live sibling, so a respin
    /// can never alter what resolution already decided (the canonical
    /// document renders deterministically, so serialized-JSON
    /// equality via `json_extract` is exact for every honest client;
    /// `IS NOT` keeps the absent-field `NULL`s comparable).  `links`
    /// is one-way: a sibling without a table never constrains, but a
    /// links-bearing sibling must be matched exactly - identities
    /// may be stamped onto a published version once, then never
    /// changed or removed by a respin.  Zero
    /// changed rows sends the glue back to [`EXISTING_REVISIONS`] to
    /// answer no-op, opt-in conflict, invariance conflict, or twin
    /// `400` from what it re-reads.
    INSERT_REVISION =
        "INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json, \
         published_at, archive_size, published_by, verification) \
         SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'pending' WHERE EXISTS \
         (SELECT 1 FROM versions WHERE scope = ?1 AND name = ?2 AND version = ?3) \
         AND NOT EXISTS \
             (SELECT 1 FROM revisions WHERE scope = ?1 AND name = ?2 AND version = ?3 \
              AND revision = ?4) \
         AND (?10 OR NOT EXISTS \
              (SELECT 1 FROM revisions WHERE scope = ?1 AND name = ?2 AND version = ?3 \
               AND verification IN ('pending', 'verified') AND checksum <> ?5)) \
         AND NOT EXISTS \
             (SELECT 1 FROM revisions WHERE scope = ?1 AND name = ?2 AND version = ?3 \
              AND verification IN ('pending', 'verified') \
              AND (json_extract(metadata_json, '$.dependencies') \
                       IS NOT json_extract(?6, '$.dependencies') \
                   OR json_extract(metadata_json, '$.features') \
                       IS NOT json_extract(?6, '$.features') \
                   OR json_extract(metadata_json, '$.standards') \
                       IS NOT json_extract(?6, '$.standards') \
                   OR (json_extract(metadata_json, '$.links') IS NOT NULL \
                       AND json_extract(metadata_json, '$.links') \
                           IS NOT json_extract(?6, '$.links'))))";

    /// Revives a rejected revision in place (back to `pending`),
    /// guarded on the row still being the rejected generation this
    /// request read.  The revision id derives from the bytes, so a
    /// revival is always byte-identical to what was rejected -
    /// `checksum` never changes - and the opt-in guard (`?6`, like
    /// [`INSERT_REVISION`]'s) keeps an unflagged revival from slipping
    /// a different-bytes revision back beside a live one.  A revival
    /// re-enters the live set, so [`INSERT_REVISION`]'s
    /// resolver-metadata invariance applies identically: a rejected
    /// sibling never constrained anyone (a later revision may have
    /// changed `dependencies` freely), so the revived document (`?1`)
    /// must agree with the live siblings of *today*, opt-in or not.
    REVIVE_REJECTED_REVISION =
        "UPDATE revisions SET metadata_json = ?1, published_at = ?2, published_by = ?3, \
         verification = 'pending', verification_reason = NULL, verified_at = NULL \
         WHERE scope = ?4 AND name = ?5 AND version = ?7 AND revision = ?8 \
         AND verification = 'rejected' AND checksum = ?9 \
         AND (?6 OR NOT EXISTS \
              (SELECT 1 FROM revisions WHERE scope = ?4 AND name = ?5 AND version = ?7 \
               AND verification IN ('pending', 'verified') AND checksum <> ?9)) \
         AND NOT EXISTS \
             (SELECT 1 FROM revisions WHERE scope = ?4 AND name = ?5 AND version = ?7 \
              AND verification IN ('pending', 'verified') \
              AND (json_extract(metadata_json, '$.dependencies') \
                       IS NOT json_extract(?1, '$.dependencies') \
                   OR json_extract(metadata_json, '$.features') \
                       IS NOT json_extract(?1, '$.features') \
                   OR json_extract(metadata_json, '$.standards') \
                       IS NOT json_extract(?1, '$.standards') \
                   OR (json_extract(metadata_json, '$.links') IS NOT NULL \
                       AND json_extract(metadata_json, '$.links') \
                           IS NOT json_extract(?1, '$.links'))))";

    /// How many versions have sat `pending` for over an hour (the
    /// stuck-verifier alert).
    COUNT_STALE_PENDING =
        "SELECT COUNT(*) AS n FROM revisions WHERE verification = 'pending' \
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
        "SELECT c.scope, c.name, c.version, v.yanked, c.published_at, v.downloads \
         FROM current_revisions c \
         JOIN versions v ON v.scope = c.scope AND v.name = c.name AND v.version = c.version \
         WHERE instr(c.scope || '/' || c.name, ?1) > 0";

    /// One visible package's verified versions with the stored
    /// metadata each carries: the session package-detail read.
    /// Served-revision-only like [`CURRENT_REVISIONS_BY_PACKAGE`], so a
    /// package with none is a missing package by construction.
    VERIFIED_VERSION_DETAILS =
        "SELECT c.version, c.revision, c.metadata_json, v.yanked, c.published_at, v.downloads \
         FROM current_revisions c \
         JOIN versions v ON v.scope = c.scope AND v.name = c.name AND v.version = c.version \
         WHERE c.scope = ?1 AND c.name = ?2";

    /// Whether the package is visible at all (>= 1 verified version):
    /// the reverse-dependencies target gate.
    HAS_VERIFIED_VERSION =
        "SELECT COUNT(*) AS n FROM revisions \
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
        "SELECT scope, name, version, published_at FROM current_revisions \
         WHERE EXISTS (SELECT 1 FROM json_each(current_revisions.metadata_json, \
                       '$.dependencies') WHERE json_each.key = ?1)";

    /// The session packages listing: every *revision* of every package
    /// the user created, deterministically ordered, each with its
    /// lifecycle state, the version-level yank flag and download
    /// count, and whether it is the served current revision - the
    /// owner dashboard shows pending respins beside what is currently
    /// served.
    LIST_USER_PACKAGES =
        "SELECT r.scope, r.name, r.version, r.revision, r.verification, v.yanked, \
         r.published_at, v.downloads, \
         EXISTS(SELECT 1 FROM current_revisions c \
                WHERE c.scope = r.scope AND c.name = r.name AND c.version = r.version \
                AND c.revision = r.revision) AS is_current \
         FROM packages p \
         JOIN revisions r ON r.scope = p.scope AND r.name = p.name \
         JOIN versions v ON v.scope = r.scope AND v.name = r.name AND v.version = r.version \
         WHERE p.created_by = ?1 \
         ORDER BY r.scope, r.name, r.published_at DESC, r.version, r.revision";

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
    /// Superseded revisions keep charging - they stay fetchable.
    USER_STORED_BYTES =
        "SELECT COALESCE(SUM(archive_size), 0) AS stored_bytes \
         FROM revisions WHERE published_by = ?1 AND verification != 'rejected'";

    /// The creator's total and created-today package counts.
    USER_PACKAGE_COUNTS =
        "SELECT COUNT(*) AS package_count, \
         COALESCE(SUM(created_at >= ?2), 0) AS new_today \
         FROM packages WHERE created_by = ?1";

    /// Revisions published into one package since a cutoff (the daily
    /// per-package version quota; a respin spends it like a new
    /// version - both store a new archive).
    COUNT_PACKAGE_VERSIONS_SINCE =
        "SELECT COUNT(*) AS n FROM revisions \
         WHERE scope = ?1 AND name = ?2 AND published_at >= ?3";

    /// Whether the package row already exists (new-package quotas).
    PACKAGE_EXISTS = "SELECT COUNT(*) AS n FROM packages WHERE scope = ?1 AND name = ?2";

    /// Whether creating `(scope, name)` would collide with an existing
    /// same-scope package under `-`/`_` folding: the deterministic
    /// publish reject (`docs/architecture.md`, "Name fidelity").
    /// `REPLACE` in the query, not a normalized column - the packages
    /// table is small and this runs once per prospective publish. The
    /// self-exclusion keeps the predicate identical to the in-batch
    /// guards on [`INSERT_PACKAGE`] / [`INSERT_VERSION_ROW`].
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
         FROM revisions WHERE published_by = ?1";

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
    /// exactly when this batch's [`INSERT_REVISION`] is about to
    /// apply and no live row references the checksum yet.  The
    /// statement runs *before* the insert (mirroring
    /// [`COUNT_STORED_BYTES_ON_REVIVAL`]'s decide-then-flip shape)
    /// and repeats the insert's guards one-for-one - the same-key
    /// conjunct included - so a suppressed insert (a twin, a racing
    /// byte-identical publish, an unflagged respin, an invariance
    /// conflict) can never count bytes, and a blob some other live
    /// row already references is never counted twice.  A post-insert
    /// row-exists test could not tell this batch's row from a racing
    /// byte-identical winner's, which is why the guards read the
    /// pre-insert state instead. The CASTs
    /// here and below keep the TEXT-affinity meta value integer-shaped:
    /// D1 binds numbers as floats, and INTEGER + REAL would otherwise
    /// store "254.0", which the breaker's strict u64 parse rejects.
    COUNT_STORED_BYTES_ON_PUBLISH =
        "INSERT INTO meta (key, value) VALUES ('total_stored_bytes', \
         CASE WHEN (SELECT COUNT(*) FROM revisions \
                    WHERE checksum = ?1 AND verification != 'rejected') = 0 \
              AND EXISTS (SELECT 1 FROM versions \
                          WHERE scope = ?3 AND name = ?4 AND version = ?5) \
              AND NOT EXISTS (SELECT 1 FROM revisions \
                              WHERE scope = ?3 AND name = ?4 AND version = ?5 \
                              AND revision = ?6) \
              AND (?8 OR NOT EXISTS \
                   (SELECT 1 FROM revisions \
                    WHERE scope = ?3 AND name = ?4 AND version = ?5 \
                    AND verification IN ('pending', 'verified') AND checksum <> ?1)) \
              AND NOT EXISTS \
                  (SELECT 1 FROM revisions \
                   WHERE scope = ?3 AND name = ?4 AND version = ?5 \
                   AND verification IN ('pending', 'verified') \
                   AND (json_extract(metadata_json, '$.dependencies') \
                            IS NOT json_extract(?7, '$.dependencies') \
                        OR json_extract(metadata_json, '$.features') \
                            IS NOT json_extract(?7, '$.features') \
                        OR json_extract(metadata_json, '$.standards') \
                            IS NOT json_extract(?7, '$.standards') \
                        OR (json_extract(metadata_json, '$.links') IS NOT NULL \
                            AND json_extract(metadata_json, '$.links') \
                                IS NOT json_extract(?7, '$.links')))) \
              THEN CAST(?2 AS INTEGER) ELSE 0 END) \
         ON CONFLICT (key) DO UPDATE SET \
         value = CAST(value AS INTEGER) + \
         CASE WHEN (SELECT COUNT(*) FROM revisions \
                    WHERE checksum = ?1 AND verification != 'rejected') = 0 \
              AND EXISTS (SELECT 1 FROM versions \
                          WHERE scope = ?3 AND name = ?4 AND version = ?5) \
              AND NOT EXISTS (SELECT 1 FROM revisions \
                              WHERE scope = ?3 AND name = ?4 AND version = ?5 \
                              AND revision = ?6) \
              AND (?8 OR NOT EXISTS \
                   (SELECT 1 FROM revisions \
                    WHERE scope = ?3 AND name = ?4 AND version = ?5 \
                    AND verification IN ('pending', 'verified') AND checksum <> ?1)) \
              AND NOT EXISTS \
                  (SELECT 1 FROM revisions \
                   WHERE scope = ?3 AND name = ?4 AND version = ?5 \
                   AND verification IN ('pending', 'verified') \
                   AND (json_extract(metadata_json, '$.dependencies') \
                            IS NOT json_extract(?7, '$.dependencies') \
                        OR json_extract(metadata_json, '$.features') \
                            IS NOT json_extract(?7, '$.features') \
                        OR json_extract(metadata_json, '$.standards') \
                            IS NOT json_extract(?7, '$.standards') \
                        OR (json_extract(metadata_json, '$.links') IS NOT NULL \
                            AND json_extract(metadata_json, '$.links') \
                                IS NOT json_extract(?7, '$.links')))) \
              THEN CAST(?2 AS INTEGER) ELSE 0 END";

    /// Refunds a rejected archive's bytes exactly when the row - still
    /// pending, still holding the bytes the verdict was read against -
    /// is the checksum's sole live reference (see `src/glue.rs`,
    /// `apply_rejection`).
    REFUND_STORED_BYTES_ON_REJECTION =
        "UPDATE meta SET value = MAX(CAST(value AS INTEGER) - \
         CASE WHEN (SELECT COUNT(*) FROM revisions \
                    WHERE checksum = ?1 AND verification != 'rejected') = 1 \
              AND (SELECT verification FROM revisions \
                   WHERE scope = ?2 AND name = ?3 AND version = ?4 AND revision = ?7) \
                  = 'pending' \
              AND (SELECT checksum FROM revisions \
                   WHERE scope = ?2 AND name = ?3 AND version = ?4 AND revision = ?7) = ?1 \
              AND (SELECT published_at FROM revisions \
                   WHERE scope = ?2 AND name = ?3 AND version = ?4 AND revision = ?7) = ?6 \
              THEN CAST(?5 AS INTEGER) ELSE 0 END, 0) \
         WHERE key = 'total_stored_bytes'";

    /// Re-counts a revived rejected revision's bytes exactly when the
    /// revival is about to apply and no other live row references the
    /// checksum - a rejection refunded them (see `src/glue.rs`,
    /// `revive_rejected_revision`; revivals are byte-identical, so
    /// the checksum is the row's own).  The conditions mirror
    /// [`REVIVE_REJECTED_REVISION`]'s guards one-for-one - the
    /// opt-in (`?8`) and resolver-metadata invariance (`?9`)
    /// conjuncts included - so the counter can never gain bytes for
    /// a flip the guards refused.
    COUNT_STORED_BYTES_ON_REVIVAL =
        "UPDATE meta SET value = CAST(value AS INTEGER) + \
         CASE WHEN (SELECT verification FROM revisions \
                    WHERE scope = ?1 AND name = ?2 AND version = ?3 AND revision = ?7) \
                   = 'rejected' \
              AND (SELECT checksum FROM revisions \
                   WHERE scope = ?1 AND name = ?2 AND version = ?3 AND revision = ?7) = ?4 \
              AND (SELECT COUNT(*) FROM revisions \
                   WHERE checksum = ?5 AND verification != 'rejected') = 0 \
              AND (?8 OR NOT EXISTS \
                   (SELECT 1 FROM revisions WHERE scope = ?1 AND name = ?2 AND version = ?3 \
                    AND verification IN ('pending', 'verified') AND checksum <> ?4)) \
              AND NOT EXISTS \
                  (SELECT 1 FROM revisions \
                   WHERE scope = ?1 AND name = ?2 AND version = ?3 \
                   AND verification IN ('pending', 'verified') \
                   AND (json_extract(metadata_json, '$.dependencies') \
                            IS NOT json_extract(?9, '$.dependencies') \
                        OR json_extract(metadata_json, '$.features') \
                            IS NOT json_extract(?9, '$.features') \
                        OR json_extract(metadata_json, '$.standards') \
                            IS NOT json_extract(?9, '$.standards') \
                        OR (json_extract(metadata_json, '$.links') IS NOT NULL \
                            AND json_extract(metadata_json, '$.links') \
                                IS NOT json_extract(?9, '$.links')))) \
              THEN CAST(?6 AS INTEGER) ELSE 0 END \
         WHERE key = 'total_stored_bytes'";

    // ------------------------------------------------------------------
    // backup: the verified-artifact replication queue
    // ------------------------------------------------------------------

    /// Enqueues a just-verified version's blob for backup replication,
    /// in the same batch as [`MARK_REVISION_VERIFIED`]: the row appears
    /// exactly when the verified transition applied (the guards repeat
    /// the mark's), so the queue is recorded transactionally with the
    /// transition and a crash can never lose the work. Shared checksums
    /// collapse onto one queue row; a key already replicated re-enters
    /// harmlessly (the drain's head sees the copy and settles).
    ENQUEUE_VERIFIED_BACKUP =
        "INSERT INTO backup_pending (key, bytes, enqueued_at) \
         SELECT 'blobs/sha256/' || substr(checksum, 8), archive_size, ?6 FROM revisions \
         WHERE scope = ?1 AND name = ?2 AND version = ?3 AND revision = ?7 \
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
         AND NOT EXISTS (SELECT 1 FROM revisions \
                         WHERE checksum = ?2 AND verification = 'verified')";

    /// Live **verified** references to one blob: the drain only copies
    /// blobs the registry still serves as verified content.
    COUNT_LIVE_VERIFIED_BLOB_REFERENCES =
        "SELECT COUNT(*) AS n FROM revisions \
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

    /// The artifact route's checksum and read-gate lookup, addressed
    /// by the immutable unit: the route's filename embeds the
    /// revision, so superseded revisions stay fetchable and the
    /// verifier can fetch a *specific* pending revision.
    ARTIFACT_BY_REVISION =
        "SELECT checksum, verification FROM revisions \
         WHERE scope = ?1 AND name = ?2 AND version = ?3 AND revision = ?4";

    /// The source viewer's lookup: the checksum plus the stored archive
    /// size, which bounds the ranged read before R2 is consulted (the
    /// blob was written from the same bytes the size was recorded
    /// from). The verified filter sits in the query like
    /// [`CURRENT_REVISIONS_BY_PACKAGE`]'s, so pending, rejected, and
    /// corrupt-status rows are missing rows by construction - sessions
    /// have no verify scope, so unlike the artifact route there is no
    /// pending carve-out to branch on.
    SOURCE_VERSION_LOOKUP =
        "SELECT checksum, archive_size FROM current_revisions \
         WHERE scope = ?1 AND name = ?2 AND version = ?3";

    /// The public stats totals: verified packages, verified versions,
    /// and served downloads. `scope || '/' || name` is unambiguous -
    /// `/` is in neither grammar - and a registry with no verified
    /// versions answers all zeros.
    REGISTRY_STATS =
        "SELECT COUNT(DISTINCT c.scope || '/' || c.name) AS packages, \
         COUNT(*) AS versions, \
         COALESCE(SUM(v.downloads), 0) AS downloads \
         FROM current_revisions c \
         JOIN versions v ON v.scope = c.scope AND v.name = c.name AND v.version = c.version";

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
         AND EXISTS (SELECT 1 FROM revisions r \
                     WHERE r.scope = ?1 AND r.name = ?2 AND r.version = ?3 \
                     AND r.verification = 'verified')";

    /// Live (non-rejected) references to one blob, for reclaim.
    COUNT_LIVE_BLOB_REFERENCES =
        "SELECT COUNT(*) AS n FROM revisions \
         WHERE checksum = ?1 AND verification != 'rejected'";

    /// The governor reconciliation's authoritative live set: one size
    /// per distinct live checksum, the same shape the storage
    /// self-accounting counts (`docs/runbook.md`, "Orphaned R2 blobs").
    LIVE_BLOB_SIZES =
        "SELECT checksum, MAX(archive_size) AS size FROM revisions \
         WHERE verification != 'rejected' GROUP BY checksum";
}
