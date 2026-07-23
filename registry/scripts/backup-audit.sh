#!/usr/bin/env bash
#
# Read-only audit of the BACKUP bucket (docs/runbook.md, "Disaster
# recovery"): verified-only coverage (every verified checksum has its
# backup copy), the dump/sidecar pairing, and - when a verify token is
# available - the governor's backup and dump pools against the bucket's
# actual contents. It never deletes anything: the deployed BACKUP
# bucket's blobs/ namespace is append-only (the nightly job prunes its
# own d1/ dumps), and an object the current verified set does
# not name may be legitimate history (a pre-wipe backup, an older
# restore's blobs), so cleanup is an operator decision made per object
# with `wrangler r2 object delete` plus `scripts/governor.sh release`,
# never a bulk sweep.
#
#   scripts/backup-audit.sh          counts only
#   scripts/backup-audit.sh --keys   also list the divergent keys
#
# Requires CLOUDFLARE_API_TOKEN (R2 listing) and wrangler auth (D1).
# REGISTRY_VERIFY_TOKEN additionally compares the governor's ledger;
# without it the ledger sections are skipped.

set -euo pipefail

cd "$(dirname -- "${BASH_SOURCE[0]}")/.."

show_keys=""
if [[ "${1:-}" == "--keys" ]]; then
  show_keys=1
elif [[ -n "${1:-}" ]]; then
  echo "usage: scripts/backup-audit.sh [--keys]" >&2
  exit 1
fi

backup_bucket="cabin-registry-backup"

step() { printf '==> %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

wrangler() { npx --yes wrangler@4.112.0 "$@"; }

: "${CLOUDFLARE_API_TOKEN:?CLOUDFLARE_API_TOKEN is required to list the backup bucket}"

account_id="$(node -e '
  const text = require("fs").readFileSync("wrangler.jsonc", "utf8");
  const m = text.match(/"CF_ACCOUNT_ID":\s*"([0-9a-f]{32})"/);
  if (!m) process.exit(1);
  console.log(m[1]);
')" || fail "CF_ACCOUNT_ID not found in wrangler.jsonc"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# The full listing, cursor-paginated (unlike the wipe's delete loop,
# an audit must walk every page). One `key<TAB>size` line per object.
step "listing $backup_bucket"
api="https://api.cloudflare.com/client/v4/accounts/$account_id/r2/buckets/$backup_bucket/objects"
cursor=""
: >"$work/listing"
while :; do
  page="$(curl -fsS -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
    "$api?per_page=1000${cursor:+&cursor=$cursor}")" \
    || fail "listing $backup_bucket failed"
  cursor="$(node -e '
    const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
    if (!out.success) process.exit(1);
    for (const obj of out.result) console.error(`${obj.key}\t${obj.size}`);
    // Pagination must be explicit: a truncated page without a cursor,
    // or missing metadata entirely, fails the audit rather than
    // silently reading a partial listing as the whole bucket.
    const info = out.result_info ?? {};
    if (info.is_truncated === true) {
      if (!info.cursor) process.exit(1);
      console.log(encodeURIComponent(info.cursor));
    } else if (info.is_truncated === false) {
      console.log("");
    } else {
      process.exit(1);
    }
  ' <<<"$page" 2>>"$work/listing")" || fail "unexpected or unpaginatable R2 list response: $page"
  [[ -z "$cursor" ]] && break
done
printf '    %s object(s)\n' "$(wc -l <"$work/listing" | tr -d ' ')"

step "reading the verified checksums and queue depth from D1"
wrangler d1 execute DB --remote --json --command "
  SELECT checksum, MAX(archive_size) AS size FROM versions
  WHERE verification = 'verified' GROUP BY checksum" >"$work/verified.json" \
  || fail "the verified-checksum query failed"
wrangler d1 execute DB --remote --json --command \
  "SELECT COUNT(*) AS n FROM backup_pending" >"$work/pending.json" \
  || fail "the queue-depth query failed"

if [[ -n "${REGISTRY_VERIFY_TOKEN:-}" ]]; then
  step "reading the governor ledger snapshot"
  api_origin="${CABIN_API_ORIGIN:-https://cabinpkg.com}"
  [[ "$api_origin" == https://* ]] || fail "CABIN_API_ORIGIN must be https"
  status="$(curl -sS -o "$work/snapshot.json" -w '%{http_code}' \
    -H "Authorization: Bearer $REGISTRY_VERIFY_TOKEN" \
    "$api_origin/api/v1/admin/governor")"
  [[ "$status" == "200" ]] \
    || fail "the governor snapshot answered $status: $(cat "$work/snapshot.json")"
else
  step "REGISTRY_VERIFY_TOKEN unset; skipping the governor ledger sections"
fi

step "auditing"
node - "$work" "${show_keys:-}" <<'JS' || exit 1
const fs = require("fs");
const [work, showKeys] = process.argv.slice(2);

const listing = fs.readFileSync(`${work}/listing`, "utf8").split("\n")
  .filter(Boolean)
  .map((line) => { const [key, size] = line.split("\t"); return { key, size: Number(size) }; });
const blobs = new Map(listing.filter((o) => o.key.startsWith("blobs/sha256/"))
  .map((o) => [o.key, o.size]));
const dumps = listing.filter((o) => /^d1\/\d{4}-\d{2}-\d{2}\.sql$/.test(o.key));
const sidecars = new Set(listing.filter((o) => o.key.endsWith(".sql.sha256")).map((o) => o.key));
const strays = listing.filter((o) => !o.key.startsWith("blobs/sha256/")
  && !/^d1\/\d{4}-\d{2}-\d{2}\.sql(\.sha256)?$/.test(o.key));

const verified = JSON.parse(fs.readFileSync(`${work}/verified.json`, "utf8"))[0].results;
const pending = JSON.parse(fs.readFileSync(`${work}/pending.json`, "utf8"))[0].results[0].n;

let failed = false;
const section = (label, keys, remedy, hard) => {
  console.log(`${label}: ${keys.length}`);
  if (keys.length > 0) {
    if (remedy) console.log(`    ${remedy}`);
    if (showKeys) for (const key of keys) console.log(`        ${key}`);
    if (hard) failed = true;
  }
};

// Verified-only coverage: every currently-verified checksum must have
// its backup copy (or a queue row still working toward one), and the
// copy must be the recorded size - a truncated object under the right
// content-addressed key is not a backup.
const missing = verified
  .filter((row) => !blobs.has(`blobs/sha256/${row.checksum}`))
  .map((row) => `blobs/sha256/${row.checksum}`);
section("verified blobs missing from the backup", missing,
  `queue depth is ${pending}; if rows are stale, run scripts/backup-backfill.sh`,
  true);
const wrongSize = verified
  .filter((row) => blobs.has(`blobs/sha256/${row.checksum}`)
    && blobs.get(`blobs/sha256/${row.checksum}`) !== row.size)
  .map((row) => `blobs/sha256/${row.checksum} `
    + `(backup ${blobs.get(`blobs/sha256/${row.checksum}`)} B, recorded ${row.size} B)`);
section("backup copies whose size disagrees with the recorded archive", wrongSize,
  "a truncated or overwritten copy; re-copy via scripts/backup-backfill.sh after deleting it",
  true);

// History the current verified set does not name: legitimate under the
// append-only policy, reported because each object holds backup-pool
// ledger allowance until an operator decides otherwise.
const verifiedKeys = new Set(verified.map((row) => `blobs/sha256/${row.checksum}`));
const extras = [...blobs.keys()].filter((key) => !verifiedKeys.has(key));
section("backup blobs beyond the current verified set", extras,
  "append-only history (pre-wipe backups, older restores); not deleted by tooling");

// Dump/sidecar pairing: the sidecar is written strictly after
// validation, so a dump without one is an unvalidated leftover the
// nightly job normally deletes; a sidecar without its dump means the
// dump object was lost.
const unvalidated = dumps.filter((o) => !sidecars.has(`${o.key}.sha256`)).map((o) => o.key);
section("dumps without a validating sidecar", unvalidated,
  "unvalidated leftovers; the next nightly pass deletes or replaces them", true);
const orphanSidecars = [...sidecars]
  .filter((key) => !dumps.some((o) => `${o.key}.sha256` === key));
section("sidecars without their dump", orphanSidecars,
  "the dump object is gone; investigate before trusting that date", true);
section("keys outside the blobs/ and d1/ layouts", strays.map((o) => o.key),
  "nothing in the service writes these; investigate", true);
console.log(`dumps retained: ${dumps.length} (retention keeps 30 dailies + 12 monthly firsts)`);

// The governor's view, when a token was available: the ledger must
// never understate the bucket (upper bound of reality).
if (fs.existsSync(`${work}/snapshot.json`)) {
  const snapshot = JSON.parse(fs.readFileSync(`${work}/snapshot.json`, "utf8"));
  const pool = (name) => snapshot.storage.filter((row) => row.pool === name)
    .reduce((acc, row) => ({ bytes: acc.bytes + row.bytes, objects: acc.objects + row.objects }),
      { bytes: 0, objects: 0 });
  const backup = pool("backup");
  const dump = pool("dump");
  const blobBytes = [...blobs.values()].reduce((n, size) => n + size, 0);
  const dumpObjects = listing.filter((o) => o.key.startsWith("d1/"));
  const dumpBytes = dumpObjects.reduce((n, o) => n + o.size, 0);
  console.log(`backup pool ledger: ${backup.bytes} B / ${backup.objects} object(s); `
    + `bucket blobs/: ${blobBytes} B / ${blobs.size} object(s)`);
  console.log(`dump pool ledger:   ${dump.bytes} B / ${dump.objects} object(s); `
    + `bucket d1/:    ${dumpBytes} B / ${dumpObjects.length} object(s)`);
  if (backup.bytes < blobBytes || dump.bytes < dumpBytes) {
    console.log("    the ledger understates the bucket - it must stay an upper bound;"
      + " run scripts/backup-backfill.sh (backup pool) and investigate dumps");
    failed = true;
  }
}

process.exit(failed ? 1 : 0);
JS

echo "backup audit OK"
