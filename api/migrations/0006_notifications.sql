-- One row per mail the registry owes an account for something that
-- happened to it (src/notifications.rs; docs/runbook.md,
-- "Notifications"), written by the batch that makes the event true.
-- kind names what happened, and event tells one occurrence of the kind
-- from another for the same recipient; the three are unique together,
-- so an event recorded twice owes one mail. detail is the JSON the mail
-- is written from. The row holds no address: the cron reads the
-- recipient's notification address when it sends.
-- attempts counts the cron passes that claimed the row and attempted_at
-- is the latest one's time; failure is the error code of that pass's
-- send, NULL when it recorded none. sent_at and message_id record the
-- send the mail service accepted; failed_at closes a row the cron has
-- given up: a permanent failure sets it, and so does the claim of the
-- row's last attempt, until that attempt's send is recorded.
CREATE TABLE notifications (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    event TEXT NOT NULL,
    recipient INTEGER NOT NULL REFERENCES users,
    detail TEXT NOT NULL,
    created_at TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    attempted_at TEXT,
    failure TEXT,
    failed_at TEXT,
    sent_at TEXT,
    message_id TEXT,
    UNIQUE (kind, event, recipient)
);
