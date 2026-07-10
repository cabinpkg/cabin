#!/usr/bin/env bash
#
# Verify a `wrangler d1 export` SQL dump before it is archived: non-empty,
# contains a CREATE TABLE for every canonical registry table, and actually
# replays into an in-memory SQLite database. Used by the nightly backup
# workflow and by scripts/restore-drill.sh; see docs/runbook.md
# ("Disaster recovery").
#
#   scripts/backup-verify-dump.sh <dump.sql>

set -euo pipefail

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

dump="${1:?usage: scripts/backup-verify-dump.sh <dump.sql>}"

[[ -s "$dump" ]] || fail "dump is missing or empty: $dump"

# The canonical tables from migrations/; a dump missing any of them is not
# a usable backup.
for table in users tokens packages versions meta; do
  grep -Eq "CREATE TABLE (IF NOT EXISTS )?\"?$table\"?" "$dump" \
    || fail "dump has no CREATE TABLE for $table"
done

# Replaying the dump proves it parses and executes, not just that it greps.
# ponytail: replays the whole dump in one process; fine at dev scale,
# revisit if dumps outgrow what a CI runner comfortably replays.
sqlite3 -bail :memory: <"$dump" || fail "dump does not replay cleanly"

echo "dump OK: $dump"
