#!/usr/bin/env bash
#
# Safe diagnostics bundle (docs/runbook.md, "Logs"): the aggregate
# state an incident report or a bug thread needs, and nothing that
# must not leave the operator's terminal - no tokens or token hashes,
# no object keys or content checksums, no package names, no user data.
# Counts, modes, timestamps, and version identifiers only; every
# section names its source so a reader can go deeper with the runbook.
#
#   scripts/diagnose.sh
#
# Requires wrangler auth (D1 reads, deployments list). With
# REGISTRY_VERIFY_TOKEN set it also includes the governor's usage
# snapshot (aggregates only). Read-only.

set -euo pipefail

cd "$(dirname -- "${BASH_SOURCE[0]}")/.."
. scripts/lib.sh

step "deploy configuration (scripts/check-deploy.sh)"
if bash scripts/check-deploy.sh >/dev/null 2>&1; then
  echo "    config OK"
else
  echo "    CONFIG CHECK FAILED - run scripts/check-deploy.sh for detail"
fi
stamp="$(cat migrations/*.sql | shasum -a 256 | cut -d' ' -f1)"
if [[ "$stamp" == "$(cat migrations-applied)" ]]; then
  echo "    migrations stamp: current (deploys unblocked)"
else
  echo "    migrations stamp: PENDING - deploys stay skipped until"
  echo "    scripts/migrate.sh --remote (or a wipe) lands and is committed"
fi

step "service state (meta; docs/runbook.md \"Budget breaker and service mode\")"
wrangler d1 execute DB --remote --json --command "
  SELECT key, value FROM meta WHERE key IN
    ('service_mode', 'service_mode_reason', 'registry_generation',
     'launched', 'last_backup_at', 'last_backup_key', 'total_stored_bytes')
  ORDER BY key" |
  node -e '
    const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
    for (const row of out[0].results) console.log(`    ${row.key}: ${row.value}`);
  '

step "corpus and queue counts (D1)"
wrangler d1 execute DB --remote --json --command "
  SELECT
    (SELECT COUNT(*) FROM users) AS users,
    (SELECT COUNT(*) FROM scopes) AS scopes,
    (SELECT COUNT(*) FROM packages) AS packages,
    (SELECT COUNT(*) FROM versions) AS versions,
    (SELECT COUNT(*) FROM versions WHERE verification = 'pending') AS pending,
    (SELECT COUNT(*) FROM versions WHERE verification = 'verified') AS verified,
    (SELECT COUNT(*) FROM versions WHERE verification = 'rejected') AS rejected,
    (SELECT COUNT(*) FROM versions WHERE yanked = 1) AS yanked,
    (SELECT COUNT(*) FROM tokens) AS tokens,
    (SELECT COUNT(*) FROM backup_pending) AS backup_pending" |
  node -e '
    const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
    for (const [key, value] of Object.entries(out[0].results[0]))
      console.log(`    ${key}: ${value}`);
  '

if [[ -n "${REGISTRY_VERIFY_TOKEN:-}" ]]; then
  step "governor ledger (scripts/governor.sh usage)"
  REGISTRY_VERIFY_TOKEN="$REGISTRY_VERIFY_TOKEN" bash scripts/governor.sh usage |
    sed -n '2,$p' | sed 's/^/    /'
else
  step "governor ledger: skipped (REGISTRY_VERIFY_TOKEN unset)"
fi

step "worker deployments (wrangler deployments list)"
wrangler deployments list 2>/dev/null | sed -n '1,30s/^/    /p' ||
  echo "    (deployments list unavailable with this token)"

echo "diagnose OK"
