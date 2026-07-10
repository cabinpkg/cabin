#!/usr/bin/env bash
#
# Restore drill (docs/runbook.md, "Disaster recovery"): prove the newest D1
# dump in the backup bucket actually restores. Downloads the dump, checks
# its checksum, imports it into a scratch D1 database, compares per-table
# row counts against the live database, spot-checks one version's metadata
# JSON byte for byte, then deletes the scratch database.
#
#   scripts/restore-drill.sh dev            # or production
#
# Requires CLOUDFLARE_API_TOKEN in the environment (D1 edit to create and
# drop the scratch database, R2 read on the backup bucket). The live
# database keeps serving; only the scratch database is written to.

set -euo pipefail

cd "$(dirname -- "${BASH_SOURCE[0]}")/.."

env_name="${1:?usage: scripts/restore-drill.sh <dev|production>}"
case "$env_name" in
  dev) backup_bucket="cabin-registry-dev-backup"; live_db="cabin-registry-dev" ;;
  production) backup_bucket="cabin-registry-prod-backup"; live_db="cabin-registry-prod" ;;
  *) echo "unknown environment: $env_name (expected dev or production)" >&2; exit 1 ;;
esac
# Fixed name, never derived from input: a leftover from a crashed drill
# fails the `d1 create` below loudly instead of being silently reused.
scratch_db="cabin-registry-drill"

step() { printf '==> %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

wrangler() { npx --yes wrangler "$@"; }

# d1_rows <database-name> <sql>: the JSON `results` array of a remote query.
d1_rows() {
  wrangler d1 execute "$1" --remote --json --command "$2" |
    node -e '
      const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
      console.log(JSON.stringify(out[0].results));
    '
}

account_id="$(sed -nE 's/.*"CF_ACCOUNT_ID": "([0-9a-f]{32})".*/\1/p' wrangler.jsonc | head -1)"
[[ -n "$account_id" ]] || fail "no CF_ACCOUNT_ID in wrangler.jsonc"

tmp="$(mktemp -d)"
scratch_created=""
cleanup() {
  rm -rf "$tmp"
  if [[ -n "$scratch_created" ]]; then
    step "tearing down the scratch database"
    wrangler d1 delete "$scratch_db" --skip-confirmation \
      || echo "WARNING: could not delete $scratch_db; remove it by hand" >&2
  fi
}
trap cleanup EXIT

step "locating the newest dump in $backup_bucket/d1/"
latest="$(curl -sS --fail \
    -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
    "https://api.cloudflare.com/client/v4/accounts/$account_id/r2/buckets/$backup_bucket/objects?prefix=d1/&per_page=1000" |
  node -e '
    const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
    const keys = out.result.map((o) => o.key)
      .filter((k) => /^d1\/\d{4}-\d{2}-\d{2}\.sql\.gz$/.test(k)).sort();
    if (keys.length === 0) { console.error("no dumps in the bucket"); process.exit(1); }
    console.log(keys[keys.length - 1]);
  ')"
dump_gz="$tmp/$(basename "$latest")"
echo "    $latest"

step "downloading and checking the dump"
wrangler r2 object get "$backup_bucket/$latest" --file "$dump_gz" --remote
wrangler r2 object get "$backup_bucket/$latest.sha256" --file "$dump_gz.sha256" --remote
(cd "$tmp" && shasum -a 256 -c "$(basename "$dump_gz.sha256")")
gunzip "$dump_gz"
dump="${dump_gz%.gz}"
scripts/backup-verify-dump.sh "$dump"

step "importing into the scratch database $scratch_db"
wrangler d1 create "$scratch_db"
scratch_created=1
wrangler d1 execute "$scratch_db" --remote --yes --file "$dump"

step "comparing per-table row counts against $live_db"
tables="$(d1_rows "$live_db" \
    "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name" |
  node -e '
    const rows = JSON.parse(require("fs").readFileSync(0, "utf8"));
    for (const { name } of rows)
      if (!name.startsWith("_cf_") && !name.startsWith("sqlite_")) console.log(name);
  ')"
[[ -n "$tables" ]] || fail "no tables in the live database"
while IFS= read -r table; do
  live_n="$(d1_rows "$live_db" "SELECT COUNT(*) AS n FROM $table" |
    node -e 'console.log(JSON.parse(require("fs").readFileSync(0, "utf8"))[0].n)')"
  scratch_n="$(d1_rows "$scratch_db" "SELECT COUNT(*) AS n FROM $table" |
    node -e 'console.log(JSON.parse(require("fs").readFileSync(0, "utf8"))[0].n)')"
  printf '    %-14s live=%s restored=%s\n' "$table" "$live_n" "$scratch_n"
  # The dump predates the comparison, so a live write in between shows up
  # here; re-run the backup and the drill together if that ever trips.
  [[ "$live_n" == "$scratch_n" ]] || fail "row count mismatch in $table"
done <<<"$tables"

step "spot-checking one version's metadata JSON"
spot_sql="SELECT name, version, metadata_json FROM versions ORDER BY name, version LIMIT 1"
live_row="$(d1_rows "$live_db" "$spot_sql")"
scratch_row="$(d1_rows "$scratch_db" "$spot_sql")"
# shellcheck disable=SC2016 # the ${...} template literal is JavaScript
node -e '
  const [live, scratch] = process.argv.slice(1).map((s) => JSON.parse(s));
  if (live.length === 0) { console.error("no versions to spot-check"); process.exit(1); }
  if (scratch.length === 0 ||
      live[0].name !== scratch[0].name ||
      live[0].version !== scratch[0].version ||
      live[0].metadata_json !== scratch[0].metadata_json) {
    console.error("restored row differs from live"); process.exit(1);
  }
  const metadata = JSON.parse(scratch[0].metadata_json);
  if (metadata.name !== scratch[0].name) {
    console.error("metadata_json name does not match the row"); process.exit(1);
  }
  console.log(`    ${scratch[0].name}@${scratch[0].version}: metadata JSON parses` +
    ", matches live byte for byte");
' "$live_row" "$scratch_row"

echo "restore drill OK: $latest restores cleanly"
