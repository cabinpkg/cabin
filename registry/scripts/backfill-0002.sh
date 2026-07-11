#!/usr/bin/env bash
#
# One-shot backfill for migration 0002 (see docs/runbook.md): fills
# versions.archive_size from the R2 blob sizes, versions.published_by and
# packages.created_by from the sole existing user, and
# meta.total_stored_bytes from the distinct blob sizes. For operators who
# prefer not to wipe pre-launch; a database created after the migration
# never needs this.
#
#   scripts/backfill-0002.sh
#
# Requires CLOUDFLARE_API_TOKEN in the environment. Idempotent: re-running
# rewrites the same values.

set -euo pipefail

cd "$(dirname -- "${BASH_SOURCE[0]}")/.."

# The pre-cutover form took an environment argument; refuse it loudly
# instead of silently acting on the sole remaining deployment.
[[ $# -eq 0 ]] || { echo "usage: scripts/backfill-0002.sh (no arguments)" >&2; exit 1; }

bucket="cabin-registry-blobs"

step() { printf '==> %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

wrangler() { npx --yes wrangler "$@"; }

d1_exec() { wrangler d1 execute DB --remote --command "$1"; }

# d1_column <sql> <column>: one value per line from a remote query.
d1_column() {
  wrangler d1 execute DB --remote --json --command "$1" |
    node -e '
      const column = process.argv[1];
      const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
      for (const row of out[0].results) console.log(row[column]);
    ' "$2"
}

step "resolving the sole existing user"
users="$(d1_column "SELECT github_id FROM users" github_id)"
[[ -n "$users" ]] || fail "no users exist; nothing to attribute versions to"
[[ "$(wc -l <<<"$users")" -eq 1 ]] \
  || fail "more than one user exists; attribute published_by manually instead"
user_id="$users"
[[ "$user_id" =~ ^[0-9]+$ ]] || fail "unexpected github_id: $user_id"

step "backfilling archive_size from the R2 blob sizes"
blob="$(mktemp)"
trap 'rm -f "$blob"' EXIT
total=0
while IFS= read -r checksum; do
  [[ -z "$checksum" ]] && continue
  [[ "$checksum" =~ ^[0-9a-f]{64}$ ]] || fail "unexpected checksum: $checksum"
  # r2 object commands default to local state; this script only ever
  # targets deployed environments.
  wrangler r2 object get "$bucket/blobs/sha256/$checksum" --file "$blob" --remote
  size="$(wc -c <"$blob" | tr -d ' ')"
  printf '    %s -> %s bytes\n' "$checksum" "$size"
  d1_exec "UPDATE versions SET archive_size = $size WHERE checksum = '$checksum'"
  total=$((total + size))
done <<<"$(d1_column "SELECT DISTINCT checksum FROM versions" checksum)"

step "backfilling published_by, created_by, and total_stored_bytes"
d1_exec "UPDATE versions SET published_by = $user_id WHERE published_by = 0;
         UPDATE packages SET created_by = $user_id WHERE created_by = 0;
         UPDATE meta SET value = '$total' WHERE key = 'total_stored_bytes';"

echo "backfill OK (total_stored_bytes = $total)"
