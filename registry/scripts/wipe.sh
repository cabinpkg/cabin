#!/usr/bin/env bash
#
# Wipe and recreate the registry's data from zero (docs/runbook.md,
# "Wipe procedure (pre-launch only)"): drop and recreate the database,
# reapply all migrations, delete the primary bucket's archive blobs,
# bump the registry generation, and redeploy. Pre-launch only: the
# launch guard (scripts/launch-guard.sh) refuses once meta.launched is
# 'true'. The deployed BACKUP bucket is never touched.
#
#   scripts/wipe.sh             # the deployed registry (asks to confirm)
#   scripts/wipe.sh --local     # the local .wrangler/ state (smoke, dev)
#
# --local resets the entire local emulated state, the emulated backup
# bucket included - local state is test data, not a backup; the
# append-only invariant protects the deployed BACKUP bucket only.
#
# Remote mode requires CLOUDFLARE_API_TOKEN in the environment and
# updates wrangler.jsonc in place with the recreated database's id -
# commit that change. Set CABIN_WIPE_YES=1 to skip the confirmation
# prompt (non-interactive runs).

set -euo pipefail

cd "$(dirname -- "${BASH_SOURCE[0]}")/.."

mode="--remote"
if [[ "${1:-}" == "--local" ]]; then
  mode="--local"
elif [[ -n "${1:-}" ]]; then
  echo "usage: scripts/wipe.sh [--local]" >&2
  exit 1
fi

database="cabin-registry"
blobs_bucket="cabin-registry-blobs"

step() { printf '==> %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

wrangler() { npx --yes wrangler "$@"; }

if [[ "$mode" == "--remote" && "${CABIN_WIPE_YES:-}" != "1" ]]; then
  printf 'About to WIPE the deployed registry (%s, %s). Type "wipe" to confirm: ' \
    "$database" "$blobs_bucket"
  read -r answer
  [[ "$answer" == "wipe" ]] || fail "not confirmed"
fi

# The guard runs after the prompt, immediately before anything
# destructive, so a flag flipped while the prompt sat waiting still
# refuses. On remote it also proves the DB binding and the account's
# database named cabin-registry are the same database, so the reads
# here and the `d1 delete` below cannot diverge.
step "launch guard"
scripts/launch-guard.sh "$mode"

# The pre-wipe generation feeds the post-wipe bump: every authenticated
# response echoes x-cabin-registry-generation, so clients and smoke runs
# can tell the wipe happened (docs/runbook.md).
step "reading the pre-wipe registry generation"
old_generation="$(wrangler d1 execute DB "$mode" --json --command \
  "SELECT value FROM meta WHERE key = 'registry_generation'" |
  node -e '
    const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
    console.log(out[0].results[0].value);
  ')"
[[ "$old_generation" =~ ^[0-9]+$ ]] \
  || fail "meta.registry_generation is not numeric: '$old_generation'"
new_generation=$((old_generation + 1))

if [[ "$mode" == "--local" ]]; then
  # The local analogue of the whole remote procedure: the emulated D1
  # and R2 state under .wrangler/ simply goes away, and migrations
  # recreate the schema from zero. No config ids, no deploy.
  step "deleting the local D1 and R2 state"
  rm -rf .wrangler/state/v3/d1 .wrangler/state/v3/r2

  step "reapplying migrations from zero"
  wrangler d1 migrations apply DB --local

  step "bumping the registry generation to $new_generation"
  wrangler d1 execute DB --local --command \
    "UPDATE meta SET value = '$new_generation' WHERE key = 'registry_generation'"

  echo "local wipe OK (generation $old_generation -> $new_generation)"
  exit 0
fi

# The account id anchors the R2 REST calls below; it is a plain var in
# wrangler.jsonc (not a secret - it is in every dashboard URL).
account_id="$(node -e '
  const text = require("fs").readFileSync("wrangler.jsonc", "utf8");
  const m = text.match(/"CF_ACCOUNT_ID":\s*"([0-9a-f]{32})"/);
  if (!m) process.exit(1);
  console.log(m[1]);
')" || fail "CF_ACCOUNT_ID not found in wrangler.jsonc"
: "${CLOUDFLARE_API_TOKEN:?CLOUDFLARE_API_TOKEN is required for the R2 sweep}"

step "dropping and recreating the $database database"
wrangler d1 delete "$database" -y
wrangler d1 create "$database" >/dev/null
new_id="$(wrangler d1 list --json | node -e '
  const list = JSON.parse(require("fs").readFileSync(0, "utf8"));
  const db = list.find((db) => db.name === process.argv[1]);
  if (!db) process.exit(1);
  console.log(db.uuid || db.database_id);
' "$database")" || fail "the recreated database is missing from d1 list"
[[ "$new_id" =~ ^[0-9a-f-]{36}$ ]] || fail "unexpected database id: '$new_id'"

# The nightly dump exports whatever database D1_DATABASE_ID names, and
# the binding deploys whatever database_id names - both must be the
# recreated id before migrating and deploying (docs/runbook.md).
step "baking the new database id into wrangler.jsonc ($new_id)"
node -e '
  const fs = require("fs");
  const id = process.argv[1];
  const text = fs.readFileSync("wrangler.jsonc", "utf8");
  const next = text
    .replace(/("database_id": ")[0-9a-f-]{36}(")/, `$1${id}$2`)
    .replace(/("D1_DATABASE_ID": ")[0-9a-f-]{36}(")/, `$1${id}$2`);
  fs.writeFileSync("wrangler.jsonc", next);
' "$new_id"
[[ "$(grep -c "$new_id" wrangler.jsonc)" -eq 2 ]] \
  || fail "wrangler.jsonc does not carry the new id exactly twice; fix it by hand"

step "applying all migrations from zero"
wrangler d1 migrations apply DB --remote

# wrangler r2 object has no list or bulk mode, so the sweep drives the
# R2 REST API directly. Deleting drains the listing, so it re-fetches
# the first page until nothing matches the prefix - no cursor handling
# (opaque cursors carry URL-hostile characters). Slashes in object keys
# stay literal in the delete URL (the API requires it); every other
# component character is percent-encoded. The BACKUP bucket is
# append-only and deliberately not swept.
step "deleting blobs/ from $blobs_bucket"
api="https://api.cloudflare.com/client/v4/accounts/$account_id/r2/buckets/$blobs_bucket/objects"
deleted=0
while :; do
  page="$(curl -fsS -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
    "$api?prefix=blobs/&per_page=500")" \
    || fail "listing $blobs_bucket failed"
  keys="$(node -e '
    const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
    if (!out.success) process.exit(1);
    for (const obj of out.result)
      console.log(obj.key.split("/").map(encodeURIComponent).join("/"));
  ' <<<"$page")" || fail "unexpected R2 list response: $page"
  [[ -z "$keys" ]] && break
  while IFS= read -r key; do
    [[ -z "$key" ]] && continue
    curl -fsS -o /dev/null -X DELETE \
      -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$api/$key" \
      || fail "deleting $key failed"
    deleted=$((deleted + 1))
  done <<<"$keys"
done
printf '    deleted %s blob(s)\n' "$deleted"

step "bumping the registry generation to $new_generation"
wrangler d1 execute DB --remote --command \
  "UPDATE meta SET value = '$new_generation' WHERE key = 'registry_generation'"

step "redeploying (bakes the new database id into the bindings)"
wrangler deploy

cat <<EOF
wipe OK (generation $old_generation -> $new_generation)

Follow-ups (docs/runbook.md):
  - commit the wrangler.jsonc database-id change
  - tokens are gone: re-issue REGISTRY_VERIFY_TOKEN on /settings/tokens
    and update the GitHub secret (gh secret set REGISTRY_VERIFY_TOKEN)
  - users sign in again and re-issue their tokens
EOF
