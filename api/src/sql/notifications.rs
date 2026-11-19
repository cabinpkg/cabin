//! notifications: the mails the registry owes its accounts, and which
//! of them a cron pass may send (`src/notifications.rs`).

statements! {
    /// Owes the account of identity `(?1, ?2)` the mail of its
    /// creation, with detail `?3`, as of `?4`. It runs in the sign-in's
    /// batch after the identity upsert, so the identity resolves to the
    /// user that batch created. An account is created once: a second
    /// record of it adds no row.
    NOTIFY_ACCOUNT_CREATED =
        "INSERT OR IGNORE INTO notifications (kind, event, recipient, detail, created_at) \
         SELECT 'account_created', '', user_id, ?3, ?4 FROM identities \
         WHERE provider = ?1 AND provider_account_id = ?2";

    /// Owes the publisher of revision `(?1, ?2, ?3, ?4)` the mail of
    /// its publication. It runs last in the batch that records the
    /// revision's archive
    /// ([`MARK_REVISION_ARCHIVED`](super::publication::MARK_REVISION_ARCHIVED)
    /// and the three statements that count the change), and does
    /// nothing unless they recorded the change's job. The detail names
    /// the generation the change took, which is that job's and what
    /// [`CLAIM_NOTIFICATION`] holds the row by. An archive is recorded
    /// once per revision, and a second record of one adds no row.
    NOTIFY_REVISION_PUBLISHED =
        "INSERT OR IGNORE INTO notifications (kind, event, recipient, detail, created_at) \
         SELECT 'revision_published', \
                r.scope || '/' || r.name || '@' || r.version || '#' || r.revision, \
                r.published_by, \
                json_object('scope', r.scope, 'name', r.name, 'version', r.version, \
                            'revision', r.revision, 'at', r.archived_at, \
                            'generation', p.generation), \
                r.archived_at \
         FROM revisions r JOIN packages p ON p.scope = r.scope AND p.name = r.name \
         WHERE r.scope = ?1 AND r.name = ?2 AND r.version = ?3 AND r.revision = ?4 \
         AND (SELECT changes()) = 1";

    /// Owes account `?5` the mail of the yank (`?4` true) or un-yank
    /// of version `(?1, ?2, ?3)` at `?6`. It runs in the batch that
    /// sets the flag, directly after
    /// [`SET_VERSION_YANKED`](super::packages::SET_VERSION_YANKED) and
    /// the three statements that count the change, and does nothing
    /// unless they recorded the change's job. The event is the scope,
    /// the time, and the newest id the table held: `SET_VERSION_YANKED`
    /// counts a day's un-yanks by the first two, and the id tells
    /// apart two changes recorded in one millisecond, so each change
    /// of the flag owes its own mail. The detail names no generation:
    /// the read plane answers from the flag at once, and the mail does
    /// not wait for the publish pass.
    NOTIFY_VERSION_YANK =
        "INSERT OR IGNORE INTO notifications (kind, event, recipient, detail, created_at) \
         SELECT CASE WHEN ?4 THEN 'version_yanked' ELSE 'version_unyanked' END, \
                ?1 || '@' || ?6 || '/' || (SELECT COALESCE(MAX(id), 0) FROM notifications), \
                ?5, \
                json_object('scope', ?1, 'name', ?2, 'version', ?3, 'at', ?6), ?6 \
         WHERE (SELECT changes()) = 1";

    /// Owes the mail of a change to scope `?1`'s members at `?5`:
    /// account `?2` added with role `?6`, or removed when `?6` is
    /// NULL, by account `?3`. It goes to `?2` and to every owner the
    /// scope has after the change, each once: an owner the change
    /// added or removed is `?2`. It runs directly after
    /// [`ADD_SCOPE_MEMBER`](super::scopes::ADD_SCOPE_MEMBER) or
    /// [`REMOVE_SCOPE_MEMBER`](super::scopes::REMOVE_SCOPE_MEMBER) in
    /// one batch, and does nothing unless that changed a row. The
    /// event is [`NOTIFY_VERSION_YANK`]'s, one for all the change's
    /// mails: `ADD_SCOPE_MEMBER` counts a day's additions by its scope
    /// and time. The detail names both accounts by their login under
    /// provider `?4`.
    NOTIFY_MEMBERSHIP =
        "INSERT OR IGNORE INTO notifications (kind, event, recipient, detail, created_at) \
         SELECT CASE WHEN ?6 IS NULL THEN 'member_removed' ELSE 'member_added' END, \
                ?1 || '@' || ?5 || '/' || (SELECT COALESCE(MAX(id), 0) FROM notifications), \
                id, \
                json_object('scope', ?1, 'role', ?6, 'at', ?5, \
                  'member', (SELECT login_snapshot FROM identities \
                             WHERE user_id = ?2 AND provider = ?4), \
                  'by', (SELECT login_snapshot FROM identities \
                         WHERE user_id = ?3 AND provider = ?4)), ?5 \
         FROM users \
         WHERE (id = ?2 OR id IN ( \
           SELECT user_id FROM scope_members WHERE scope_name = ?1 AND role = 'owner')) \
         AND (SELECT changes()) = 1";

    /// Owes account `?2` the mail of its claim of scope `?1` at `?3`.
    /// It runs last in the claim's batch, after
    /// [`RECORD_SCOPE_CLAIM`](super::scopes::RECORD_SCOPE_CLAIM), and
    /// does nothing unless that recorded the claim.
    NOTIFY_SCOPE_CLAIMED =
        "INSERT OR IGNORE INTO notifications (kind, event, recipient, detail, created_at) \
         SELECT 'scope_claimed', ?1 || '@' || ?3, ?2, json_object('scope', ?1, 'at', ?3), ?3 \
         WHERE (SELECT changes()) = 1";

    /// Claims the one mail a pass sends next at `?1`, the oldest row
    /// that is owed one, counting the attempt before the send, and
    /// answers the row with the address its recipient has now. A row
    /// is owed a mail while it is neither sent nor given up and its
    /// recipient has a notification address; one without an address
    /// waits unclaimed until its owner signs in with one. Nor is a row
    /// owed its mail while the publication job of the generation its
    /// detail names is open: the publish pass has not completed that
    /// change yet (`glue::publication`). After its `n`th claim a row
    /// waits `?3` minutes doubled `n` times for the next, which is
    /// also what keeps a second pass from sending a row the first
    /// still holds. The claim clears `failure`, so until the
    /// pass records an outcome the row reads as claimed with none; and
    /// a row's `?2`th claim gives the row up, which only the record of
    /// that claim's send undoes, so a row no pass ever settles ends
    /// given up like any other. `INDEXED BY` keeps the scan to the
    /// rows neither sent nor given up: the settled rows are never
    /// deleted, and there is one for every revision ever published.
    CLAIM_NOTIFICATION =
        "UPDATE notifications SET attempts = attempts + 1, attempted_at = ?1, failure = NULL, \
           failed_at = CASE WHEN attempts + 1 >= ?2 THEN ?1 END \
         WHERE id = ( \
           SELECT n.id FROM notifications n INDEXED BY notifications_unsettled \
           JOIN users u ON u.id = n.recipient \
           WHERE n.sent_at IS NULL AND n.failed_at IS NULL \
             AND u.notification_email IS NOT NULL \
             AND NOT EXISTS (SELECT 1 FROM publication_jobs j \
                             WHERE j.generation = n.detail ->> '$.generation' \
                             AND j.completed_at IS NULL) \
             AND (n.attempted_at IS NULL \
                  OR n.attempted_at <= strftime('%Y-%m-%dT%H:%M:%fZ', ?1, \
                                                '-' || (?3 << n.attempts) || ' minutes')) \
           ORDER BY n.id LIMIT 1) \
         RETURNING id, kind, detail, \
           (SELECT notification_email FROM users WHERE id = recipient) AS address";

    /// Records that the mail service accepted row `?1`'s mail at `?2`
    /// as message `?3`. It runs only after the send returned, so a pass
    /// that dies between the two leaves the row unsent and its next
    /// claim sends the mail again.
    MARK_NOTIFICATION_SENT =
        "UPDATE notifications SET sent_at = ?2, message_id = ?3, failed_at = NULL WHERE id = ?1";

    /// Records that row `?1`'s send failed with code `?2`, and gives the
    /// row up as of `?4` when the failure is permanent (`?3`). A row
    /// whose last claim this was is given up already.
    MARK_NOTIFICATION_FAILED =
        "UPDATE notifications SET failure = ?2, \
           failed_at = COALESCE(failed_at, CASE WHEN ?3 THEN ?4 END) \
         WHERE id = ?1";

    /// What a check from outside the cron reads of the delivery pass,
    /// as of `?1`, over the rows [`CLAIM_NOTIFICATION`] may still
    /// claim: neither sent nor given up, their recipient has an
    /// address, and no open publication job holds them. A row is due
    /// from its creation, and after its `n`th claim from `?2` minutes
    /// doubled `n` times past that claim. `waiting_seconds` is how
    /// long the row due longest has been due: 0 when none is due yet,
    /// NULL when there is no such row. `attempts` is the most claims
    /// any of them has had. `sent_seconds` is the time since a pass
    /// last recorded a mail as sent, which it writes under the `meta`
    /// key `?3`: NULL when none has.
    NOTIFICATION_DELIVERY_STATE =
        "SELECT MAX(0, unixepoch(?1) - unixepoch(MIN( \
                  CASE WHEN n.attempted_at IS NULL THEN n.created_at \
                       ELSE strftime('%Y-%m-%dT%H:%M:%fZ', n.attempted_at, \
                                     '+' || (?2 << n.attempts) || ' minutes') END))) \
                  AS waiting_seconds, \
                COALESCE(MAX(n.attempts), 0) AS attempts, \
                (SELECT unixepoch(?1) - unixepoch(value) FROM meta WHERE key = ?3) \
                  AS sent_seconds \
         FROM notifications n INDEXED BY notifications_unsettled \
         JOIN users u ON u.id = n.recipient \
         WHERE n.sent_at IS NULL AND n.failed_at IS NULL \
           AND u.notification_email IS NOT NULL \
           AND NOT EXISTS (SELECT 1 FROM publication_jobs j \
                           WHERE j.generation = n.detail ->> '$.generation' \
                           AND j.completed_at IS NULL)";
}
