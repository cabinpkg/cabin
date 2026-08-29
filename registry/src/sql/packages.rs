//! packages/versions: the read plane, publish, yank, verification.

statements! {
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
    /// package ([`TWIN_PACKAGE_EXISTS`](super::quota::TWIN_PACKAGE_EXISTS) is the preflight that renders
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
}
