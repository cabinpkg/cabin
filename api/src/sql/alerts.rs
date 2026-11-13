//! alerts: what the cron observed of each alert kind, and which rows
//! are owed a mail (`src/alerts.rs`).

statements! {
    /// Records a kind firing at level `?2` (1 or more). A kind leaving
    /// level 0 starts over: `since` moves, and `notified_level` returns
    /// to 0, so the firing is mailed whatever was mailed before (a
    /// recovery still owed a mail is dropped for it). Otherwise a level
    /// below the one last mailed lowers `notified_level` with it, so a
    /// later rise is mailed as a rise. A changed level starts its mail
    /// attempts over.
    FIRE_ALERT =
        "INSERT INTO alerts (kind, level, since, summary) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT (kind) DO UPDATE SET \
           level = excluded.level, \
           since = CASE WHEN alerts.level = 0 THEN excluded.since ELSE alerts.since END, \
           summary = excluded.summary, \
           notified_level = CASE WHEN alerts.level = 0 THEN 0 \
             ELSE MIN(alerts.notified_level, excluded.level) END, \
           attempts = CASE WHEN alerts.level = excluded.level THEN alerts.attempts ELSE 0 END";

    /// Records a kind healthy again, as of `?2`. The summary stays: the
    /// recovery mail says what recovered.
    CLEAR_ALERT =
        "UPDATE alerts SET level = 0, since = ?2, attempts = 0 WHERE kind = ?1 AND level > 0";

    /// Claims the rows owed a mail for the pass at `?1`, counting the
    /// attempt before the mail is sent. A row is owed one when its
    /// level differs from `notified_level` (a first firing, a rise, a
    /// recovery), or when it is at level `?4` or above and its
    /// last mail is older than the modifier `?2`. A row whose mail has
    /// failed `?3` times running waits the same `?2` for its next try.
    CLAIM_ALERTS =
        "UPDATE alerts SET attempts = attempts + 1, attempted_at = ?1 \
         WHERE (level <> notified_level \
                OR (level >= ?4 AND notified_at <= strftime('%Y-%m-%dT%H:%M:%fZ', ?1, ?2))) \
           AND (attempts < ?3 OR attempted_at <= strftime('%Y-%m-%dT%H:%M:%fZ', ?1, ?2)) \
         RETURNING kind, level, notified_level, since, summary";

    /// Records that the mail of the pass at `?3` told the operator kind
    /// `?1` is at level `?2`: the level the pass claimed, which the row
    /// may have left since. A row that has fallen since without
    /// recovering keeps the lower level, so its next rise is mailed. A
    /// row another pass has claimed since carries that pass's time and
    /// is left to it.
    MARK_ALERT_NOTIFIED =
        "UPDATE alerts SET \
           notified_level = CASE WHEN level > 0 THEN MIN(?2, level) ELSE ?2 END, \
           notified_at = ?3, attempts = 0 \
         WHERE kind = ?1 AND attempted_at = ?3";
}
