#!/usr/bin/env bash
#
# One-shot backup reconciliation (see docs/runbook.md, "Disaster
# recovery"): copies every **verified** archive blob that is missing
# from the BACKUP bucket. The deployed Worker replicates through the
# durable backup_pending queue on its own; this script is the manual
# recovery path for a drain that keeps failing, or for seeding backups
# over pre-existing data.
#
# It UPSERTS one backup_pending queue row per verified checksum and
# never deletes any: the Worker's drain retires each row itself (its
# existence head finds the copy, settles the governor's backup ledger
# at the observed size, and deletes the row), so every copy made here -
# and every pre-queue verified blob - is absorbed, ledger included,
# within one breaker cron pass. Deleting or skipping rows here would
# leave the governor's backup ledger understating reality.
#
#   scripts/backup-backfill.sh
#
# Requires CLOUDFLARE_API_TOKEN in the environment. Idempotent:
# re-running skips blobs the backup already holds. Note the copies run
# outside the Worker, so they are not charged to the governor's
# operation pools (they bill as ordinary R2 usage on the operator's
# account activity).

set -euo pipefail

cd "$(dirname -- "${BASH_SOURCE[0]}")/.."

# The pre-cutover form took an environment argument; refuse it loudly
# instead of silently acting on the sole remaining deployment.
[[ $# -eq 0 ]] || { echo "usage: scripts/backup-backfill.sh (no arguments)" >&2; exit 1; }

primary="cabin-registry-blobs"
backup="cabin-registry-backup"

step() { printf '==> %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

wrangler() { npx --yes wrangler@4.112.0 "$@"; }

# d1_column <sql> <column>: one value per line from a remote query.
d1_column() {
  wrangler d1 execute DB --remote --json --command "$1" |
    node -e '
      const column = process.argv[1];
      const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
      for (const row of out[0].results) console.log(row[column]);
    ' "$2"
}

# The queue rows make the drain visit (and ledger) every verified
# blob, whether this run copies it or an earlier out-of-band copy
# already exists. MAX(archive_size) is the conservative expected size;
# the drain settles at the size its head observes.
step "enqueueing every verified blob for the worker's drain"
wrangler d1 execute DB --remote --command "
  INSERT INTO backup_pending (key, bytes, enqueued_at)
    SELECT 'blobs/sha256/' || checksum, MAX(archive_size),
           strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    FROM versions WHERE verification = 'verified' GROUP BY checksum
  ON CONFLICT (key) DO NOTHING" >/dev/null

step "copying verified blobs missing from $backup"
blob="$(mktemp)"
trap 'rm -f "$blob"' EXIT
# Captured via an assignment first: a failed enumeration aborts here
# (`set -e` catches failing assignments), whereas a command substitution
# inside `<<<` would silently feed the loop nothing. Verified only: the
# backup set holds exactly the content the registry serves as verified
# (docs/architecture.md, "Backups"); pending uploads are not backed up
# until their verdict, and rejected blobs are reclaimed.
checksums="$(d1_column "SELECT DISTINCT checksum FROM versions
  WHERE verification = 'verified'" checksum)"
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
done <<<"$checksums"

echo "backup backfill OK (copied $copied, already present $present)"
