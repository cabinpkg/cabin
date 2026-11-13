//! auth/tokens: bearer-token verification, token management, users -
//! plus the login sessions' short-lived human credential.

statements! {
    /// The bearer-token lookup; the effective quota class is the token
    /// row's own when set (the trustpub exchange's granted tier), else
    /// the owning user's. Revoked tokens never match. Neither do expired
    /// ones (`?2` is the current ISO-8601 instant): enforcing expiry
    /// inside this WHERE makes an expired token produce the exact no-row
    /// result an unknown hash does - same uniform 401, same single
    /// lookup - so no response or timing oracle separates "expired" from
    /// "invalid". A token is live only from `created_at` on: the
    /// trustpub lifetime ceiling anchors on `created_at`, so a minting
    /// bug forging a far-future anchor must yield a row that cannot
    /// authenticate before its anchor, not one that outruns the cap.
    /// `github_id` is the owning account's GitHub identity (NULL when it
    /// has none), which the glue re-checks against the private-alpha
    /// allowlist on every use (`crate::allowlist::admits`).
    AUTH_TOKEN_LOOKUP =
        "SELECT t.id, t.user_id, t.scopes, \
                COALESCE(t.quota_class, u.quota_class) AS quota_class, t.scope_limit, \
                u.quota_class AS user_quota_class, u.rl_tokens, u.rl_updated_at, \
                (SELECT i.provider_account_id FROM identities i \
                 WHERE i.user_id = u.id AND i.provider = 'github') AS github_id \
         FROM tokens t JOIN users u ON u.id = t.user_id \
         WHERE t.token_hash = ?1 AND t.revoked_at IS NULL \
         AND t.created_at <= ?2 AND t.expires_at > ?2";

    /// Best-effort `last_used_at` bookkeeping on every
    /// bearer-authenticated request.
    TOUCH_TOKEN_LAST_USED = "UPDATE tokens SET last_used_at = ?1 WHERE id = ?2";

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

    /// An external identity's registry-native user row.
    USER_BY_IDENTITY =
        "SELECT i.user_id, i.login_snapshot, u.quota_class \
         FROM identities i JOIN users u ON u.id = i.user_id \
         WHERE i.provider = ?1 AND i.provider_account_id = ?2";

    /// The browser session's user resolution, run on every session-plane
    /// request: [`USER_BY_IDENTITY`] for the sealed cookie's account,
    /// returning a row only while the cookie's `browser_sessions` row
    /// (`?3`) exists for that same account - a logged-out cookie gets the
    /// same no-row answer as an unknown identity, in the same single
    /// read.
    SESSION_USER =
        "SELECT i.user_id, i.login_snapshot, u.quota_class \
         FROM browser_sessions s \
         JOIN identities i ON i.provider = ?1 AND i.provider_account_id = s.provider_account_id \
         JOIN users u ON u.id = i.user_id \
         WHERE s.id = ?3 AND s.provider_account_id = ?2";

    /// Records a browser session at sign-in, in the identity upsert's
    /// batch.
    INSERT_BROWSER_SESSION =
        "INSERT INTO browser_sessions (id, provider_account_id, expires_at) VALUES (?1, ?2, ?3)";

    /// Lazy cleanup ridden on every sign-in batch (`?1` is the current
    /// Unix second): a row past its cookie's sealed expiry can no longer
    /// authenticate anything.
    PRUNE_EXPIRED_BROWSER_SESSIONS = "DELETE FROM browser_sessions WHERE expires_at <= ?1";

    /// Logout: revokes exactly the presented cookie's session. Both the
    /// id and the account come from the MAC-verified cookie, so a caller
    /// can only ever delete a session it holds the cookie for.
    DELETE_BROWSER_SESSION =
        "DELETE FROM browser_sessions WHERE id = ?1 AND provider_account_id = ?2";

    /// Mints a login-session token (`PUT /api/v1/sessions/tokens`); D1
    /// stores only the SHA-256 hex of the plaintext. The shape is fixed
    /// here and re-enforced by the schema's session CHECK: bounded,
    /// unconfined, and NULL `quota_class` so the row inherits the owning
    /// user's class live through [`AUTH_TOKEN_LOOKUP`]'s COALESCE. The
    /// scope set is `publish,yank`, plus `verify` only for the operator's
    /// own account - `users.id` 1, the row the baseline migration seeds
    /// as the operator. Quota classes stay resource tiers: promoting
    /// another account to 'operator' raises its limits and nothing else.
    /// The CHECK admits both strings; which user gets which is decided
    /// here alone.
    INSERT_SESSION_TOKEN =
        "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, \
                             expires_at, kind) \
         VALUES (?1, ?2, 'login session', ?3, \
                 CASE WHEN ?2 = 1 THEN 'publish,yank,verify' ELSE 'publish,yank' END, \
                 ?4, ?5, 'session')";

    /// [`DELETE_TRUSTPUB_TOKEN`](super::trustpub::DELETE_TRUSTPUB_TOKEN)'s
    /// session sibling, with the same zero-changes-is-401 discipline.
    DELETE_SESSION_TOKEN = "DELETE FROM tokens WHERE id = ?1 AND kind = 'session'";
}
