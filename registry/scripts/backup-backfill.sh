#!/usr/bin/env bash
#
# One-shot backup reconciliation (see docs/runbook.md, "Disaster
# recovery"): copies every referenced archive blob that is missing from
# the BACKUP bucket, then clears the blob-replication failure log.
# Publish-time replication is best-effort, so run this once after
# enabling backups for an environment with existing data, and again
# whenever the breaker cron alerts on replication failures.
#
#   scripts/backup-backfill.sh dev            # or production
#
# Requires CLOUDFLARE_API_TOKEN in the environment. Idempotent:
# re-running skips blobs the backup already holds.

set -euo pipefail

cd "$(dirname -- "${BASH_SOURCE[0]}")/.."

env_name="${1:?usage: scripts/backup-backfill.sh <dev|production>}"
case "$env_name" in
  dev) primary="cabin-registry-dev-blobs" backup="cabin-registry-dev-backup" ;;
  production) primary="cabin-registry-prod-blobs" backup="cabin-registry-prod-backup" ;;
  *) echo "unknown environment: $env_name (expected dev or production)" >&2; exit 1 ;;
esac

step() { printf '==> %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

wrangler() { npx --yes wrangler "$@"; }

d1_exec() { wrangler d1 execute DB --env "$env_name" --remote --command "$1" >/dev/null; }

# d1_column <sql> <column>: one value per line from a remote query.
d1_column() {
  wrangler d1 execute DB --env "$env_name" --remote --json --command "$1" |
    node -e '
      const column = process.argv[1];
      const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
      for (const row of out[0].results) console.log(row[column]);
    ' "$2"
}

step "copying referenced blobs missing from $backup"
blob="$(mktemp)"
trap 'rm -f "$blob"' EXIT
# Captured via an assignment first: a failed enumeration aborts here
# (`set -e` catches failing assignments), whereas a command substitution
# inside `<<<` would silently feed the loop nothing - and the failure
# log below must never be cleared on a run that enumerated nothing.
# Rejected rows are excluded: their blob is reclaimed from the primary
# (docs/architecture.md, "The verification lifecycle"), so there is
# nothing to copy and no backup need.
checksums="$(d1_column "SELECT DISTINCT checksum FROM versions
  WHERE verification != 'rejected'" checksum)"
copied=0
present=0
while IFS= read -r checksum; do
  [[ -z "$checksum" ]] && continue
  [[ "$checksum" =~ ^[0-9a-f]{64}$ ]] || fail "unexpected checksum: $checksum"
  key="blobs/sha256/$checksum"
  # r2 object commands default to local state; this script only ever
  # targets deployed environments.
  if wrangler r2 object get "$backup/$key" --file "$blob" --remote >/dev/null 2>&1; then
    present=$((present + 1))
  else
    wrangler r2 object get "$primary/$key" --file "$blob" --remote
    wrangler r2 object put "$backup/$key" --file "$blob" --remote
    printf '    copied %s (%s bytes)\n' "$key" "$(wc -c <"$blob" | tr -d ' ')"
    copied=$((copied + 1))
  fi
  # Clear the failure log for exactly this verified key. Never the
  # whole table: a publish racing this run can log a failure for a
  # blob that postdates the snapshot above, and erasing that row would
  # silence the breaker alert while the blob is still missing.
  d1_exec "DELETE FROM backup_replication_failures WHERE key = '$key'"
done <<<"$checksums"

step "clearing failure rows for blobs with no live reference"
# The reclaim path clears these as it deletes, but a failed bookkeeping
# write can leave a straggler that would alert forever with no primary
# object left to copy. Rows for live checksums stay untouched.
d1_exec "DELETE FROM backup_replication_failures WHERE key NOT IN
  (SELECT 'blobs/sha256/' || checksum FROM versions WHERE verification != 'rejected')"

echo "backup backfill OK (copied $copied, already present $present)"
