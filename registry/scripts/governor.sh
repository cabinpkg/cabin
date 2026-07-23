#!/usr/bin/env bash
#
# Operator surface for the cost governor's ledger (docs/runbook.md,
# "The cost governor"): the guarded spellings of the admin endpoint's
# actions, so ledger maintenance never starts from hand-typed curl.
#
#   scripts/governor.sh usage                inspect the ledger snapshot
#   scripts/governor.sh compare              ledger totals vs D1's live view
#   scripts/governor.sh reconcile [--keys]   on-demand increase-only rebuild
#   scripts/governor.sh release <pool> <key> evidence-checked entry release
#   scripts/governor.sh wipe                 pre-launch ledger reset (guarded)
#
# Every action needs REGISTRY_VERIFY_TOKEN (a verify-scoped token;
# docs/runbook.md, "Verification pipeline"). `release` and `wipe` also
# need CLOUDFLARE_API_TOKEN: their guards prove object absence through
# the R2 REST API before touching the ledger, because a release for an
# object that still exists would make the ledger understate reality -
# the one direction the design forbids. `usage`, `compare`, and
# `reconcile` are safe on a live registry (reconcile is increase-only);
# `wipe` is the registry wipe's companion and refuses once launched.
# CABIN_API_ORIGIN overrides the API origin for scratch rehearsal
# deployments (default https://cabinpkg.com; https only).

set -euo pipefail

cd "$(dirname -- "${BASH_SOURCE[0]}")/.."
. scripts/lib.sh

api_origin="${CABIN_API_ORIGIN:-https://cabinpkg.com}"
endpoint="$api_origin/api/v1/admin/governor"
primary_bucket="cabin-registry-blobs"
backup_bucket="cabin-registry-backup"

[[ "$api_origin" == https://* ]] || fail "CABIN_API_ORIGIN must be https"
: "${REGISTRY_VERIFY_TOKEN:?REGISTRY_VERIFY_TOKEN (verify scope) is required}"

body="$(mktemp)"
trap 'rm -f "$body" "$body.snapshot" "$body.d1"' EXIT

# api <method> [json]: calls the admin governor endpoint; the response
# body lands in $body and the status is echoed.
api() {
  local method="$1" data="${2:-}"
  if [[ -n "$data" ]]; then
    curl -sS -o "$body" -w '%{http_code}' -X "$method" \
      -H "Authorization: Bearer $REGISTRY_VERIFY_TOKEN" \
      -H "Content-Type: application/json" -d "$data" "$endpoint"
  else
    curl -sS -o "$body" -w '%{http_code}' \
      -H "Authorization: Bearer $REGISTRY_VERIFY_TOKEN" "$endpoint"
  fi
}

require_api_ok() { # <status> <what>
  [[ "$1" == "200" ]] || fail "$2 answered $1: $(cat "$body")"
}

account_id() {
  node -e '
    const text = require("fs").readFileSync("wrangler.jsonc", "utf8");
    const m = text.match(/"CF_ACCOUNT_ID":\s*"([0-9a-f]{32})"/);
    if (!m) process.exit(1);
    console.log(m[1]);
  ' || fail "CF_ACCOUNT_ID not found in wrangler.jsonc"
}

# The R2 evidence helpers run inside `if` conditions, where bash
# suppresses errexit - so no step below may rely on `set -e`, and
# every failure must be told apart from "absent": an API or parse
# failure aborts via fail (exit leaves the script even from a
# condition context), because absence must be proven, never inferred
# from an error.

# r2_list_page <bucket> <prefix>: one page of the bucket listing under
# the prefix, on stdout; non-zero on any API failure.
r2_list_page() {
  local bucket="$1" prefix="$2"
  : "${CLOUDFLARE_API_TOKEN:?CLOUDFLARE_API_TOKEN is required for the R2 evidence check}"
  curl -fsS -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
    "https://api.cloudflare.com/client/v4/accounts/$(account_id)/r2/buckets/$bucket/objects?prefix=$(node -e '
      console.log(process.argv[1].split("/").map(encodeURIComponent).join("/"));
    ' "$prefix")&per_page=5"
}

# r2_key_exists <bucket> <key>: 0 when the exact key exists, 1 when the
# listing affirmatively lacks it; anything unprovable aborts.
r2_key_exists() {
  local bucket="$1" key="$2" page found=0
  page="$(r2_list_page "$bucket" "$key")" \
    || fail "listing $bucket failed; cannot prove absence"
  node -e '
    let out;
    try { out = JSON.parse(require("fs").readFileSync(0, "utf8")); }
    catch { process.exit(2); }
    if (!out || out.success !== true || !Array.isArray(out.result)) process.exit(2);
    process.exit(out.result.some((obj) => obj.key === process.argv[1]) ? 0 : 1);
  ' "$key" <<<"$page" || found=$?
  [[ "$found" == 1 ]] && return 1
  [[ "$found" == 0 ]] && return 0
  fail "unexpected R2 list response: $page"
}

# r2_prefix_nonempty <bucket> <prefix>: 0 when at least one object
# lives under the prefix, 1 when the listing is affirmatively empty;
# anything unprovable aborts.
r2_prefix_nonempty() {
  local bucket="$1" prefix="$2" page nonempty=0
  page="$(r2_list_page "$bucket" "$prefix")" \
    || fail "listing $bucket failed; cannot prove emptiness"
  node -e '
    let out;
    try { out = JSON.parse(require("fs").readFileSync(0, "utf8")); }
    catch { process.exit(2); }
    if (!out || out.success !== true || !Array.isArray(out.result)) process.exit(2);
    process.exit(out.result.length > 0 ? 0 : 1);
  ' <<<"$page" || nonempty=$?
  [[ "$nonempty" == 1 ]] && return 1
  [[ "$nonempty" == 0 ]] && return 0
  fail "unexpected R2 list response: $page"
}

print_snapshot() {
  node -e '
    const s = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
    console.log("storage (bytes are the ledger, an upper bound of R2):");
    for (const row of s.storage)
      console.log(`    ${row.pool}/${row.state}: ${row.bytes} B in ${row.objects} object(s)`);
    if (s.storage.length === 0) console.log("    (empty)");
    console.log("ops (used of the UTC-month window):");
    for (const row of s.ops) console.log(`    ${row.pool}[${row.window}]: ${row.used}`);
    if (s.ops.length === 0) console.log("    (no window opened yet)");
  ' "$body"
}

# The live service mode, for the write-coordination gates below.
service_mode() {
  wrangler d1 execute DB --remote --json --command \
    "SELECT value FROM meta WHERE key = 'service_mode'" |
    node -e '
      const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
      const results = out[0].results;
      console.log(results.length === 0 ? "__MISSING__" : String(results[0].value));
    ' || fail "could not read meta.service_mode"
}

print_report() { # [--keys]
  node -e '
    const report = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
    const keys = process.argv[2] === "--keys";
    const show = (label, list) => {
      console.log(`    ${label}: ${list.length}`);
      if (keys) for (const key of list) console.log(`        ${key}`);
    };
    show("added (previously unledgered, now committed)", report.added);
    show("unreferenced (candidate orphans; release needs evidence)", report.unreferenced);
    show("mismatched (ledger kept the larger byte count)", report.mismatched);
  ' "$body" "${1:-}"
}

command="${1:-}"
case "$command" in
  usage)
    step "governor usage snapshot ($endpoint)"
    require_api_ok "$(api GET)" "the usage snapshot"
    print_snapshot
    ;;

  compare)
    # Totals only, deliberately: the snapshot is aggregate, and the
    # key-level divergence list is `reconcile`'s report (increase-only,
    # safe on a live registry).
    step "governor usage snapshot"
    require_api_ok "$(api GET)" "the usage snapshot"
    cp "$body" "$body.snapshot"
    step "D1's authoritative view (live and verified blob totals)"
    wrangler d1 execute DB --remote --json --command "
      SELECT
        (SELECT COUNT(*) FROM (SELECT checksum FROM versions
          WHERE verification != 'rejected' GROUP BY checksum)) AS live_objects,
        (SELECT COALESCE(SUM(size), 0) FROM (SELECT MAX(archive_size) AS size
          FROM versions WHERE verification != 'rejected' GROUP BY checksum)) AS live_bytes,
        (SELECT COUNT(*) FROM (SELECT checksum FROM versions
          WHERE verification = 'verified' GROUP BY checksum)) AS verified_objects,
        (SELECT COALESCE(SUM(size), 0) FROM (SELECT MAX(archive_size) AS size
          FROM versions WHERE verification = 'verified' GROUP BY checksum)) AS verified_bytes
    " >"$body.d1" || fail "the D1 totals query failed"
    node -e '
      const fs = require("fs");
      const s = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
      const d1 = JSON.parse(fs.readFileSync(process.argv[2], "utf8"))[0].results[0];
      const pool = (name) => {
        const rows = s.storage.filter((row) => row.pool === name);
        return {
          bytes: rows.reduce((n, row) => n + row.bytes, 0),
          objects: rows.reduce((n, row) => n + row.objects, 0),
        };
      };
      const primary = pool("primary"), backup = pool("backup"), dump = pool("dump");
      console.log(`primary ledger: ${primary.bytes} B in ${primary.objects} object(s)`);
      console.log(`D1 live view:   ${d1.live_bytes} B in ${d1.live_objects} blob(s)`);
      if (primary.bytes < d1.live_bytes || primary.objects < d1.live_objects)
        console.log("    ledger understates D1: run scripts/governor.sh reconcile");
      if (primary.objects > d1.live_objects)
        console.log(
          "    ledger holds entries D1 does not prove live: candidate orphans, or a\n" +
          "    pre-wipe ledger if a registry wipe just ran (not proof by itself - an\n" +
          "    empty registry looks the same); scripts/governor.sh reconcile lists keys");
      console.log(`backup ledger:  ${backup.bytes} B in ${backup.objects} object(s)`);
      console.log(`D1 verified:    ${d1.verified_bytes} B in ${d1.verified_objects} blob(s)`);
      console.log(
        "    (the backup pool may legitimately exceed the verified view: the BACKUP\n" +
        "    bucket is append-only and keeps history; scripts/backup-audit.sh audits it)");
      console.log(`dump ledger:    ${dump.bytes} B in ${dump.objects} object(s)`);
      console.log(
        "    (audit the d1/ prefix against this with scripts/backup-audit.sh)");
    ' "$body.snapshot" "$body.d1"
    rm -f "$body.snapshot" "$body.d1"
    ;;

  reconcile)
    step "on-demand increase-only reconcile (primary pool from D1)"
    require_api_ok "$(api POST '{"reconcile":true}')" "the reconcile"
    print_report "${2:-}"
    echo "reconcile OK (operation windows, backup, and dump accounting are"
    echo "not touched; docs/runbook.md, \"Known ceilings\")"
    ;;

  release)
    pool="${2:-}"
    key="${3:-}"
    case "$pool" in
      primary)
        [[ "$key" =~ ^blobs/sha256/[0-9a-f]{64}$ ]] \
          || fail "a primary key looks like blobs/sha256/<64 hex>"
        bucket="$primary_bucket"
        ;;
      dump)
        [[ "$key" =~ ^d1/[0-9]{4}-[0-9]{2}-[0-9]{2}\.sql(\.sha256)?$ ]] \
          || fail "a dump key looks like d1/<YYYY-MM-DD>.sql[.sha256]"
        bucket="$backup_bucket"
        ;;
      backup)
        # Deleting from the append-only BACKUP bucket is an incident
        # action, never routine maintenance - the extra confirmation
        # marks the boundary, and the evidence rule is the same.
        [[ "$key" =~ ^blobs/sha256/[0-9a-f]{64}$ ]] \
          || fail "a backup key looks like blobs/sha256/<64 hex>"
        bucket="$backup_bucket"
        if [[ "${CABIN_GOVERNOR_RELEASE_BACKUP_YES:-}" != "1" ]]; then
          printf 'Backup-pool accounting is append-only; releasing marks an incident, not maintenance. Type "release-backup" to confirm: '
          read -r answer
          [[ "$answer" == "release-backup" ]] || fail "not confirmed"
        fi
        ;;
      *)
        fail "usage: scripts/governor.sh release <primary|backup|dump> <key>"
        ;;
    esac
    if [[ "$pool" == "primary" ]]; then
      # Observation alone cannot close the race with an in-flight
      # publisher (reserve taken, R2 put not yet landed: the key reads
      # absent now and appears after the release - unledgered spend
      # reconciliation can never discover, because D1 never references
      # it either). Coordination does: with writes blocked and the
      # in-flight window drained, no publisher can put after the check
      # (docs/runbook.md, "The cost governor" has the exact sequence).
      step "evidence: writes are blocked (no publisher can race the release)"
      mode="$(service_mode)"
      case "$mode" in
        writes_blocked | reads_blocked) ;;
        *) fail "service_mode is '$mode'; block writes first, wait out the in-flight window, then release (docs/runbook.md, \"The cost governor\")" ;;
      esac
    fi
    step "evidence: $key must be absent from $bucket"
    if r2_key_exists "$bucket" "$key"; then
      fail "$key still exists in $bucket; a release for a live object would make the ledger understate reality"
    fi
    if [[ "$pool" == "primary" ]]; then
      step "evidence: no non-rejected D1 version references the checksum"
      refs="$(wrangler d1 execute DB --remote --json --command "
        SELECT COUNT(*) AS n FROM versions
        WHERE verification != 'rejected'
          AND checksum = '${key#blobs/sha256/}'" |
        node -e '
          const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
          console.log(out[0].results[0].n);
        ')" || fail "the D1 reference check failed"
      [[ "$refs" == "0" ]] \
        || fail "$refs live D1 version(s) still reference this checksum; reconciliation would re-add the entry"
    fi
    if [[ "$pool" == "primary" ]]; then
      # The breaker cron overwrites a manual service-mode override
      # within 15 minutes; re-check immediately before the release so
      # the coordination cannot have silently lapsed during the
      # evidence steps above.
      mode="$(service_mode)"
      case "$mode" in
        writes_blocked | reads_blocked) ;;
        *) fail "service_mode reverted to '$mode' during the evidence checks (the breaker cron restores it); re-apply the override and re-run" ;;
      esac
    fi
    step "releasing $pool $key"
    require_api_ok "$(api POST "{\"release\":{\"pool\":\"$pool\",\"key\":\"$key\"}}")" "the release"
    # The evidence checks race a concurrent same-checksum write by
    # nature (the endpoint cannot inspect R2 atomically), so the window
    # is closed from the other side: re-check, and if the key came back
    # the ledger is repaired immediately instead of waiting for a cron.
    step "post-release verification: the key is still absent"
    if r2_key_exists "$bucket" "$key"; then
      echo "WARNING: $key reappeared in $bucket inside the release window." >&2
      case "$pool" in
        primary)
          step "repairing: reconcile re-adds every D1-referenced object now"
          require_api_ok "$(api POST '{"reconcile":true}')" "the repair reconcile"
          echo "verify with scripts/governor.sh compare before moving on" >&2
          ;;
        backup)
          echo "run scripts/backup-backfill.sh so the drain re-ledgers it" >&2
          ;;
        dump)
          echo "the nightly dump job re-commits its objects; audit with scripts/backup-audit.sh" >&2
          ;;
      esac
      exit 1
    fi
    echo "release OK"
    ;;

  wipe)
    # Mirrors scripts/wipe.sh: confirmation first, the guard immediately
    # before the destructive call so a flag flipped while the prompt sat
    # waiting still refuses.
    if [[ "${CABIN_GOVERNOR_WIPE_YES:-}" != "1" ]]; then
      printf 'About to WIPE the governor ledger'\''s primary rows (pre-launch only). Type "governor-wipe" to confirm: '
      read -r answer
      [[ "$answer" == "governor-wipe" ]] || fail "not confirmed"
    fi
    step "launch guard"
    scripts/launch-guard.sh --remote
    # A delayed publisher (request in flight since before the registry
    # wipe) could still land a put after the emptiness check below.
    # The freshly-wiped database normally holds no publish-capable
    # token at all - proving that (or blocked writes) closes the race.
    step "evidence: no publish-capable token exists, or writes are blocked"
    publishers="$(wrangler d1 execute DB --remote --json --command \
      "SELECT COUNT(*) AS n FROM tokens WHERE scopes LIKE '%publish%'" |
      node -e '
        const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
        console.log(out[0].results[0].n);
      ')" || fail "could not count publish-capable tokens"
    if [[ "$publishers" != "0" ]]; then
      mode="$(service_mode)"
      case "$mode" in
        writes_blocked | reads_blocked) ;;
        *) fail "$publishers publish-capable token(s) exist and service_mode is '$mode'; block writes first (docs/runbook.md, \"Budget breaker and service mode\")" ;;
      esac
    fi
    # The ledger wipe is the registry wipe's step 7: with primary blobs
    # still present, wiping the ledger would undercount objects that
    # keep billing (reconciliation cannot see them - the wiped D1 no
    # longer references them).
    step "evidence: $primary_bucket carries no blobs/ objects"
    if r2_prefix_nonempty "$primary_bucket" "blobs/"; then
      fail "$primary_bucket still holds blobs/ objects; finish scripts/wipe.sh first"
    fi
    if [[ "$publishers" != "0" ]]; then
      # The breaker cron can restore the mode during the R2 checks
      # above; re-check immediately before the wipe posts, exactly
      # like the release path.
      mode="$(service_mode)"
      case "$mode" in
        writes_blocked | reads_blocked) ;;
        *) fail "service_mode reverted to '$mode' during the evidence checks (the breaker cron restores it); re-apply the override and re-run" ;;
      esac
    fi
    step "wiping the governor's primary rows and fairness windows"
    require_api_ok "$(api POST '{"wipe":true}')" "the ledger wipe"
    # The emptiness check races a concurrent writer by nature; re-check
    # after the wipe so an interleaved write cannot leave the fresh
    # ledger silently undercounting (pre-launch the only credentialed
    # writer is the operator running this, so a hit here means the
    # registry wipe was incomplete or something unexpected is writing).
    step "post-wipe verification: $primary_bucket still carries no blobs/ objects"
    if r2_prefix_nonempty "$primary_bucket" "blobs/"; then
      echo "WARNING: blobs/ objects appeared in $primary_bucket during the wipe window;" >&2
      echo "finish scripts/wipe.sh, investigate the writer, and re-run scripts/governor.sh wipe" >&2
      exit 1
    fi
    echo "governor wipe OK (backup, dump, and the monthly op windows survive"
    echo "on purpose; docs/runbook.md, \"The cost governor\")"
    ;;

  *)
    echo "usage: scripts/governor.sh <usage|compare|reconcile [--keys]|release <pool> <key>|wipe>" >&2
    exit 1
    ;;
esac
