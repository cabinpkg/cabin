#!/usr/bin/env bash
#
# Restore drill (see docs/runbook.md, "Disaster recovery"): proves the
# latest nightly dump actually restores. Downloads the dump named by
# meta.last_backup_key from the BACKUP bucket, verifies its sidecar
# checksum, imports it into a scratch D1 database, compares per-table
# row counts against the live database, spot-checks one version's
# metadata JSON byte-for-byte, and tears the scratch database down.
# Run it after enabling backups and again whenever the dump machinery
# changes. Row counts can legitimately drift on an active database
# (the dump is from the last nightly pass); on a quiet registry they
# match exactly.
#
#   scripts/restore-drill.sh
#
# Requires CLOUDFLARE_API_TOKEN in the environment. The scratch
# database is cabin-registry-drill; the script refuses to run when one
# already exists (a previous failed drill - inspect, then delete it).

set -euo pipefail

cd "$(dirname -- "${BASH_SOURCE[0]}")/.."

# The pre-cutover form took an environment argument; refuse it loudly
# instead of silently acting on the sole remaining deployment.
[[ $# -eq 0 ]] || { echo "usage: scripts/restore-drill.sh (no arguments)" >&2; exit 1; }

backup="cabin-registry-backup"
scratch="cabin-registry-drill"

step() { printf '==> %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

wrangler() { npx --yes wrangler "$@"; }

# column <database-arg...> -- <sql> <column>: one value per line.
column() {
  local args=()
  while [[ "$1" != "--" ]]; do args+=("$1"); shift; done
  shift
  wrangler d1 execute "${args[@]}" --remote --json --command "$1" |
    node -e '
      const column = process.argv[1];
      const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
      for (const row of out[0].results) console.log(row[column] ?? "");
    ' "$2"
}

live_column() { column DB -- "$1" "$2"; }
scratch_column() { column "$scratch" -- "$1" "$2"; }

work="$(mktemp -d)"
created_scratch=""
cleanup() {
  [[ -n "$created_scratch" ]] && wrangler d1 delete "$scratch" -y >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

step "resolving the latest dump from meta.last_backup_key"
key="$(live_column "SELECT value FROM meta WHERE key = 'last_backup_key'" value)"
[[ "$key" =~ ^d1/[0-9]{4}-[0-9]{2}-[0-9]{2}\.sql$ ]] \
  || fail "meta.last_backup_key is missing or malformed: '$key' (has a dump run?)"
dump_name="${key#d1/}"

step "downloading $key and its checksum sidecar from $backup"
wrangler r2 object get "$backup/$key" --file "$work/$dump_name" --remote
wrangler r2 object get "$backup/$key.sha256" --file "$work/$dump_name.sha256" --remote
(cd "$work" && shasum -a 256 -c "$dump_name.sha256") \
  || fail "dump checksum verification failed"

step "creating the scratch database $scratch"
if wrangler d1 list --json | node -e '
  const list = JSON.parse(require("fs").readFileSync(0, "utf8"));
  process.exit(list.some((db) => db.name === process.argv[1]) ? 0 : 1);
' "$scratch"; then
  fail "$scratch already exists (a previous drill?); inspect and delete it first"
fi
wrangler d1 create "$scratch" >/dev/null
created_scratch=1

step "importing the dump into $scratch"
wrangler d1 execute "$scratch" --remote --file "$work/$dump_name" -y

step "comparing per-table row counts against the live database"
# Enumerated from the LIVE database on purpose: a table the dump failed
# to carry then fails the scratch-side count below, instead of silently
# never being compared.
tables="$(live_column "SELECT name FROM sqlite_master WHERE type = 'table'
  AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
  AND name NOT LIKE '\\_cf\\_%' ESCAPE '\\' ORDER BY name" name)"
[[ -n "$tables" ]] || fail "the live database contains no tables"
mismatch=0
while IFS= read -r table; do
  [[ "$table" =~ ^[A-Za-z0-9_-]+$ ]] || fail "unexpected table name: $table"
  count_sql="SELECT COUNT(*) AS n FROM \"$table\""
  if [[ "$table" == "meta" ]]; then
    # The dump is exported before the job records its own success, so
    # the live last_backup_at / last_backup_key rows are legitimately
    # newer than the dump (and absent entirely from the first one);
    # exclude them from the comparison.
    count_sql="SELECT COUNT(*) AS n FROM meta
      WHERE key NOT IN ('last_backup_at', 'last_backup_key')"
  fi
  live_n="$(live_column "$count_sql" n)"
  scratch_n="$(scratch_column "$count_sql" n)"
  marker=""
  [[ "$live_n" == "$scratch_n" ]] || { marker=" <- MISMATCH"; mismatch=1; }
  printf '    %-28s live %6s  restored %6s%s\n' "$table" "$live_n" "$scratch_n" "$marker"
done <<<"$tables"
[[ "$mismatch" -eq 0 ]] \
  || fail "row counts differ (drift since the dump, or an incomplete restore - compare timestamps)"

step "spot-checking one version's metadata JSON"
spot_sql="SELECT name || '@' || version AS pin, metadata_json
  FROM versions ORDER BY name, version LIMIT 1"
live_pin="$(live_column "$spot_sql" pin)"
if [[ -z "$live_pin" ]]; then
  echo "    no versions in the live database; nothing to spot-check"
else
  scratch_pin="$(scratch_column "$spot_sql" pin)"
  [[ "$live_pin" == "$scratch_pin" ]] \
    || fail "spot-check row differs: live $live_pin, restored $scratch_pin"
  live_column "$spot_sql" metadata_json >"$work/live.json"
  scratch_column "$spot_sql" metadata_json >"$work/restored.json"
  cmp -s "$work/live.json" "$work/restored.json" \
    || fail "metadata_json for $live_pin differs between live and restored"
  node -e 'JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"))' \
    "$work/restored.json" || fail "restored metadata_json for $live_pin is not valid JSON"
  printf '    %s: metadata_json matches and parses (%s bytes)\n' \
    "$live_pin" "$(wc -c <"$work/restored.json" | tr -d ' ')"
fi

step "tearing down $scratch"
wrangler d1 delete "$scratch" -y >/dev/null
created_scratch=""

echo "restore drill OK ($key)"
