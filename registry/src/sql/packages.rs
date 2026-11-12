//! packages/versions: the read plane, publish, yank, verification.

statements! {
    /// The package document's per-version base rows: each served
    /// version's **current** revision (the `current_revisions` view is
    /// the single "served revision" definition) with the version-level
    /// yank flag.  Pending and rejected revisions, and verified ones
    /// not yet in ARCHIVES, never reach composition, and a respin
    /// leaves the previously served revision in place here until its
    /// own archive is placed.  `CROSS JOIN` makes `versions` the outer
    /// loop (SQLite never reorders it), so the read seeks the package
    /// by primary key: the registry never runs `ANALYZE`, and without
    /// `sqlite_stat1` the planner drives the flattened view from
    /// `revisions_verification` instead, reading every verified
    /// revision in the registry on each anonymous request.
    CURRENT_REVISIONS_BY_PACKAGE =
        "SELECT c.version, c.revision, c.metadata_json, v.yanked \
         FROM versions v \
         CROSS JOIN current_revisions c \
           ON c.scope = v.scope AND c.name = v.name AND c.version = v.version \
         WHERE v.scope = ?1 AND v.name = ?2";

    /// Every **served** revision of the package (verified, its archive
    /// in ARCHIVES), for the composed document's per-version
    /// `revisions` maps: superseded revisions stay listed (and
    /// fetchable) so pinned lockfiles keep building, and their stored
    /// documents give them their own `resolution`.
    VERIFIED_REVISIONS_BY_PACKAGE =
        "SELECT version, revision, checksum, published_at, metadata_json FROM revisions \
         WHERE scope = ?1 AND name = ?2 AND verification = 'verified' \
         AND archived_at IS NOT NULL \
         ORDER BY version, revision";

    /// The yank handler's current-state read: the version-level yank
    /// flag plus whether any revision is served (yank applies to
    /// versions the registry actually serves).
    VERSION_YANK_STATE =
        "SELECT v.yanked, EXISTS(SELECT 1 FROM revisions r \
         WHERE r.scope = v.scope AND r.name = v.name AND r.version = v.version \
         AND r.verification = 'verified' AND r.archived_at IS NOT NULL) AS verified \
         FROM versions v WHERE v.scope = ?1 AND v.name = ?2 AND v.version = ?3";

    /// Applies a yank or un-yank; the `yanked` column is the single home
    /// of yank state. It changes a row only when the flag changes, which
    /// is what the statements after it in the batch count
    /// ([`ADVANCE_CATALOG_GENERATION`](super::publication::ADVANCE_CATALOG_GENERATION)).
    SET_VERSION_YANKED =
        "UPDATE versions SET yanked = ?1 \
         WHERE scope = ?2 AND name = ?3 AND version = ?4 AND yanked != ?1";

    /// The verifier's deterministic work list, filtered by status.
    /// One row per revision: a pending respin lists beside the
    /// version's already-verified revisions without disturbing them.
    /// A reservation - pending, its archive not yet confirmed stored -
    /// is not work yet and is never listed.
    REVISIONS_BY_VERIFICATION_STATUS =
        "SELECT scope, name, version, revision, checksum, published_by, published_at, \
         metadata_json \
         FROM revisions WHERE verification = ?1 \
         AND (verification != 'pending' OR stored_at IS NOT NULL) \
         ORDER BY scope, name, version, revision";

    /// The admin corpus listing (`docs/remote-registry.md`, "Admin API
    /// (scope `verify`)"): every package with whether any of its
    /// versions is **verified** - the verifier's name advisories
    /// compare a candidate against every existing name, and skip a
    /// candidate whose name was accepted once. Deliberately
    /// verified-only, not any-verdict: a rejection must never vet a
    /// name, or an operator rejecting an abstained squat would exempt
    /// that very name's next version from the advisories.
    ADMIN_PACKAGES =
        "SELECT p.scope, p.name, \
         EXISTS(SELECT 1 FROM revisions r \
                WHERE r.scope = p.scope AND r.name = p.name \
                AND r.verification = 'verified') AS vetted \
         FROM packages p ORDER BY p.scope, p.name";

    /// The verdict handler's read of the revision row a verdict
    /// targets (the body's required checksum names the revision). A
    /// reservation was never listed, so it is no target.
    VERDICT_TARGET =
        "SELECT verification, checksum, published_at FROM revisions \
         WHERE scope = ?1 AND name = ?2 AND version = ?3 AND revision = ?4 \
         AND (verification != 'pending' OR stored_at IS NOT NULL)";

    /// Applies a `verified` verdict, guarded on the revision row still
    /// being the pending generation the verdict names (the listing's
    /// `checksum` and `published_at`, which the handler's own read agreed
    /// with).
    MARK_REVISION_VERIFIED =
        "UPDATE revisions SET verification = 'verified', verified_at = ?1 \
         WHERE scope = ?2 AND name = ?3 AND version = ?4 AND revision = ?7 \
         AND verification = 'pending' AND checksum = ?5 \
         AND published_at = ?6";

    /// Applies a `rejected` verdict (`?8` the time) under the same
    /// generation guards.
    MARK_REVISION_REJECTED =
        "UPDATE revisions SET verification = 'rejected', verification_reason = ?1, \
         verified_at = NULL, rejected_at = ?8 \
         WHERE scope = ?2 AND name = ?3 AND version = ?4 AND revision = ?7 \
         AND verification = 'pending' AND checksum = ?5 AND published_at = ?6";

    /// The publish handler's read of every existing revision of
    /// `(scope, name, version)` with its upload operation
    /// (`crate::upload::dispose`), deterministic for the re-read after
    /// a lost in-batch race. `resumable` is whether the operation was
    /// first charged within `?5` of `?4` (the request's clock sample
    /// and `crate::upload::RETRY_WINDOW`).
    EXISTING_REVISIONS =
        "SELECT revision, checksum, verification, upload_id, published_at, published_by, \
         stored_at IS NOT NULL AS stored, \
         COALESCE(charged_at >= strftime('%Y-%m-%dT%H:%M:%fZ', ?4, ?5), 0) AS resumable \
         FROM revisions \
         WHERE scope = ?1 AND name = ?2 AND version = ?3 ORDER BY revision";

    /// Whether the token's user is a member (any role) of the scope: the
    /// write plane's authorization read. A scope that does not exist has
    /// no members, so nonexistent and foreign scopes answer identically
    /// by construction.
    SCOPE_MEMBERSHIP =
        "SELECT COUNT(*) AS n FROM scope_members WHERE scope_name = ?1 AND user_id = ?2";

    /// How many stored uploads have sat `pending` for over an hour (the
    /// stuck-verifier alert). `stored_at` is in the ISO 8601 shape the
    /// cutoff is rendered in, so the comparison stays lexicographic.
    COUNT_STALE_PENDING =
        "SELECT COUNT(*) AS n FROM revisions WHERE verification = 'pending' \
         AND stored_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 hour')";

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
    /// breaker's `d1_rows_read_day` budget is the tripwire. Exhausting
    /// that budget blocks writes, so both scans stay off
    /// unauthenticated routes.
    SEARCH_VERIFIED_VERSIONS =
        "SELECT c.scope, c.name, c.version, v.yanked, c.published_at, v.downloads \
         FROM current_revisions c \
         JOIN versions v ON v.scope = c.scope AND v.name = c.name AND v.version = c.version \
         WHERE instr(c.scope || '/' || c.name, ?1) > 0";

    /// The catalog (`GET /catalog.json`): every scope's verified,
    /// non-yanked versions with their current revision's stored
    /// metadata, grouped and ordered in host-testable Rust
    /// (`documents::catalog_json`).  A scan of the verified corpus like
    /// [`SEARCH_VERIFIED_VERSIONS`], so it stays off unauthenticated
    /// routes: the read plane's private-alpha admission check covers
    /// it, and opening that plane to anonymous reads must first take
    /// the scan off the request path - the edge cache spares only hits,
    /// and concurrent misses each scan.
    CATALOG_VERSIONS =
        "SELECT c.scope, c.name, c.version, c.revision, c.metadata_json \
         FROM current_revisions c \
         JOIN versions v ON v.scope = c.scope AND v.name = c.name AND v.version = c.version \
         WHERE v.yanked = 0";

    /// One visible package's verified versions with the stored
    /// metadata each carries: the session package-detail read.
    /// Served-revision-only like [`CURRENT_REVISIONS_BY_PACKAGE`], so a
    /// package with none is a missing package by construction.
    /// `CROSS JOIN` for the reason [`CURRENT_REVISIONS_BY_PACKAGE`]
    /// gives.
    VERIFIED_VERSION_DETAILS =
        "SELECT c.version, c.revision, c.metadata_json, v.yanked, c.published_at, v.downloads \
         FROM versions v \
         CROSS JOIN current_revisions c \
           ON c.scope = v.scope AND c.name = v.name AND c.version = v.version \
         WHERE v.scope = ?1 AND v.name = ?2";

    /// Whether the package is visible at all (>= 1 served version):
    /// the reverse-dependencies target gate.  The unary `+` keeps the
    /// planner off `revisions_verification`, which without
    /// `sqlite_stat1` it would otherwise prefer to the primary key.
    HAS_VERIFIED_VERSION =
        "SELECT COUNT(*) AS n FROM revisions \
         WHERE scope = ?1 AND name = ?2 AND +verification = 'verified' \
         AND archived_at IS NOT NULL";

    /// The verified versions whose stored `dependencies` map contains
    /// the canonical `<scope>/<name>` key in `?1`: a `json_each` walk
    /// over every verified row's `metadata_json` per call. That full
    /// scan is acceptable: at current scale it sits well inside the D1
    /// budget the breaker watches (`d1_rows_read_day`), and the upgrade
    /// path - a publish-maintained dependents table, the crates.io
    /// approach - is to be taken only if that metric warns, not
    /// preemptively. Dependent packages are visible by construction: a
    /// verified matching version is itself the dependent's visibility.
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
