-- The launch flag (docs/runbook.md, "Data policy"): 'false' while the
-- registry's data is disposable (pre-launch), flipped to 'true' exactly
-- once, by hand, as a launch-checklist item. Every destructive
-- maintenance path (scripts/launch-guard.sh) reads it first and refuses
-- while it is 'true'.
INSERT OR IGNORE INTO meta (key, value) VALUES ('launched', 'false');
