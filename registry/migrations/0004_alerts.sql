-- One row per alert kind the cron has seen firing (src/alerts.rs;
-- docs/runbook.md, "Alerts"). level is 0 once the kind is healthy
-- again. notified_level is the level the next mail is decided against:
-- the level the operator's last mail named, lowered without a mail
-- when the level falls below it, and 0 again when the kind fires from
-- level 0. src/sql/alerts.rs decides from the two what is owed a mail.
-- since is when the kind left level 0, or returned to it. attempts
-- counts the passes in a row whose mail for this row failed.
CREATE TABLE alerts (
    kind TEXT PRIMARY KEY,
    level INTEGER NOT NULL,
    since TEXT NOT NULL,
    summary TEXT NOT NULL,
    notified_level INTEGER NOT NULL DEFAULT 0,
    notified_at TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    attempted_at TEXT
);
