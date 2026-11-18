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

    /// Claims the one mail a pass sends next at `?1`, the oldest row
    /// that is owed one, counting the attempt before the send, and
    /// answers the row with the address its recipient has now. A row
    /// is owed a mail while it is neither sent nor given up and its
    /// recipient has a notification address; one without an address
    /// waits unclaimed until its owner signs in with one. After its
    /// `n`th claim a row waits `?3` minutes doubled `n` times for the
    /// next, which is also what keeps a second pass from sending a row
    /// the first still holds. The claim clears `failure`, so until the
    /// pass records an outcome the row reads as claimed with none; and
    /// a row's `?2`th claim gives the row up, which only the record of
    /// that claim's send undoes, so a row no pass ever settles ends
    /// given up like any other.
    // shortcut: the scan passes every sent row, and none is pruned yet;
    // index the unsettled rows or prune once the table is large.
    CLAIM_NOTIFICATION =
        "UPDATE notifications SET attempts = attempts + 1, attempted_at = ?1, failure = NULL, \
           failed_at = CASE WHEN attempts + 1 >= ?2 THEN ?1 END \
         WHERE id = ( \
           SELECT n.id FROM notifications n JOIN users u ON u.id = n.recipient \
           WHERE n.sent_at IS NULL AND n.failed_at IS NULL \
             AND u.notification_email IS NOT NULL \
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
}
